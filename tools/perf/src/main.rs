use std::error::Error;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::StreamExt as _;
use s3_wire::{
    ByteStream, Credentials, DeleteObjectRequest, Endpoint, GetObjectRequest, HeadObjectRequest,
    ManagedMultipartUploadRequest, MultipartOptions, ObjectKey, PutObjectRequest, S3Client,
    S3Config, StaticCredentialsProvider,
};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

type AnyError = Box<dyn Error + Send + Sync>;
type Result<T> = std::result::Result<T, AnyError>;

const DEFAULT_PAYLOAD_MIB: u64 = 64;
const DEFAULT_LARGE_FILE_MIB: u64 = 256;
const DEFAULT_PART_MIB: u64 = 8;
const DEFAULT_CONCURRENCY: usize = 4;
const MIB: u64 = 1024 * 1024;
const FILE_GENERATION_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct Options {
    payload_bytes: u64,
    large_file_bytes: u64,
    part_bytes: u64,
    concurrency: usize,
    json_path: PathBuf,
    markdown_path: PathBuf,
}

#[derive(Serialize)]
struct Report {
    schema_version: u8,
    generated_at_utc: String,
    generated_at_unix_seconds: u64,
    environment: Environment,
    workload: Workload,
    measurements: Measurements,
    caveats: Vec<&'static str>,
}

#[derive(Serialize)]
struct Environment {
    git_revision: String,
    git_worktree_dirty: bool,
    rustc: String,
    cargo: String,
    target_arch: &'static str,
    target_os: &'static str,
    kernel: String,
    cpu_model: String,
    logical_cpus: usize,
    total_memory_bytes: Option<u64>,
    docker_server: String,
    minio_release: String,
    minio_image: String,
    endpoint: String,
    build_profile: String,
}

#[derive(Serialize)]
struct Workload {
    payload_bytes: u64,
    payload_pattern: &'static str,
    multipart_part_bytes: u64,
    multipart_concurrency: usize,
    multipart_in_flight_budget_bytes: u64,
    disk_backed_peak_rss_limit_bytes: u64,
    large_file_bytes: u64,
    file_generation_buffer_bytes: usize,
    measured_iterations_per_transfer: u8,
    warmup: &'static str,
}

#[derive(Serialize)]
struct Measurements {
    cold_client_construction: Timing,
    disk_backed_managed_multipart_upload: Timing,
    disk_backed_rss_bound: BoundCheck,
    single_put: Timing,
    streaming_get_to_sink: Timing,
    managed_multipart_upload: Timing,
    sampled_process_peak_rss_bytes: Option<u64>,
}

#[derive(Serialize)]
struct BoundCheck {
    observed_peak_rss_delta_bytes: Option<u64>,
    limit_bytes: u64,
    passed: bool,
    rationale: &'static str,
}

#[derive(Serialize)]
struct Timing {
    elapsed_seconds: f64,
    bytes: Option<u64>,
    throughput_mib_per_second: Option<f64>,
    rss_before_bytes: Option<u64>,
    peak_rss_bytes: Option<u64>,
    peak_rss_delta_bytes: Option<u64>,
    peak_rss_delta_to_object_size: Option<f64>,
}

impl Timing {
    fn operation(elapsed: Duration, rss: RssReading) -> Self {
        Self {
            elapsed_seconds: elapsed.as_secs_f64(),
            bytes: None,
            throughput_mib_per_second: None,
            rss_before_bytes: rss.before,
            peak_rss_bytes: rss.peak,
            peak_rss_delta_bytes: rss.delta(),
            peak_rss_delta_to_object_size: None,
        }
    }

    fn transfer(elapsed: Duration, bytes: u64, rss: RssReading) -> Self {
        let seconds = elapsed.as_secs_f64();
        let peak_rss_delta_bytes = rss.delta();
        Self {
            elapsed_seconds: seconds,
            bytes: Some(bytes),
            throughput_mib_per_second: Some(bytes as f64 / MIB as f64 / seconds),
            rss_before_bytes: rss.before,
            peak_rss_bytes: rss.peak,
            peak_rss_delta_bytes,
            peak_rss_delta_to_object_size: peak_rss_delta_bytes
                .map(|delta| delta as f64 / bytes as f64),
        }
    }
}

struct RssSampler {
    stop: Arc<AtomicBool>,
    phase_peak: Arc<AtomicU64>,
    process_peak: Arc<AtomicU64>,
    handle: Option<thread::JoinHandle<()>>,
}

#[derive(Clone, Copy)]
struct RssReading {
    before: Option<u64>,
    peak: Option<u64>,
}

impl RssReading {
    fn delta(self) -> Option<u64> {
        Some(self.peak?.saturating_sub(self.before?))
    }
}

impl RssSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let phase_peak = Arc::new(AtomicU64::new(current_rss_bytes().unwrap_or(0)));
        let process_peak = Arc::new(AtomicU64::new(current_rss_bytes().unwrap_or(0)));
        let worker_stop = Arc::clone(&stop);
        let worker_peak = Arc::clone(&phase_peak);
        let worker_process_peak = Arc::clone(&process_peak);
        let handle = thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                if let Some(rss) = current_rss_bytes() {
                    worker_peak.fetch_max(rss, Ordering::Relaxed);
                    worker_process_peak.fetch_max(rss, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            phase_peak,
            process_peak,
            handle: Some(handle),
        }
    }

    fn begin_phase(&self) -> Option<u64> {
        let before = current_rss_bytes();
        self.phase_peak
            .store(before.unwrap_or(0), Ordering::Relaxed);
        before
    }

    fn finish_phase(&self, before: Option<u64>) -> RssReading {
        if let Some(rss) = current_rss_bytes() {
            self.phase_peak.fetch_max(rss, Ordering::Relaxed);
            self.process_peak.fetch_max(rss, Ordering::Relaxed);
        }
        let peak = self.phase_peak.load(Ordering::Relaxed);
        RssReading {
            before,
            peak: (peak != 0).then_some(peak),
        }
    }

    fn sampled_process_peak(&self) -> Option<u64> {
        if let Some(rss) = current_rss_bytes() {
            self.process_peak.fetch_max(rss, Ordering::Relaxed);
        }
        let peak = self.process_peak.load(Ordering::Relaxed);
        (peak != 0).then_some(peak)
    }
}

impl Drop for RssSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = parse_options()?;
    let endpoint = required_env("MINIO_S3_ENDPOINT")?;
    let bucket = required_env("MINIO_S3_BUCKET")?;
    let access_key = required_env("MINIO_ROOT_USER")?;
    let secret_key = required_env("MINIO_ROOT_PASSWORD")?;
    let credentials = Credentials::new(access_key, secret_key, None)?;
    let config = S3Config::builder()
        .endpoint(Endpoint::new(endpoint.clone())?)
        .allow_http_for_local_testing()
        .bucket(bucket)
        .region("us-east-1")
        .credentials_provider(Arc::new(StaticCredentialsProvider::new(credentials)))
        .attempt_timeout(Duration::from_secs(600))
        .operation_timeout(Duration::from_secs(600))
        .idle_body_timeout(Duration::from_secs(60))
        .build()?;
    let multipart_options = MultipartOptions::new(options.part_bytes, options.concurrency)?
        .with_transfer_timeout(Duration::from_secs(600))?;

    let rss = RssSampler::start();
    let before = rss.begin_phase();
    let started = Instant::now();
    let client = S3Client::new(config)?;
    let construction = Timing::operation(started.elapsed(), rss.finish_phase(before));

    let namespace = format!(
        "s3-wire-measurements/{}/{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    );
    let warmup_key = object_key(format!("{namespace}/warmup"))?;
    client
        .put_object(PutObjectRequest::new(
            warmup_key.clone(),
            ByteStream::from_bytes(Bytes::from_static(b"warmup")),
        ))
        .await?;
    client
        .delete_object(DeleteObjectRequest::new(warmup_key))
        .await?;

    let large_file = generate_deterministic_file(options.large_file_bytes)?;
    let large_file_key = object_key(format!("{namespace}/disk-backed-managed-multipart"))?;
    let before = rss.begin_phase();
    let started = Instant::now();
    client
        .multipart_upload(
            ManagedMultipartUploadRequest::from_path(large_file_key.clone(), large_file.path())
                .with_options(multipart_options),
        )
        .await?;
    let disk_backed_multipart = Timing::transfer(
        started.elapsed(),
        options.large_file_bytes,
        rss.finish_phase(before),
    );
    let uploaded = client
        .head_object(HeadObjectRequest::new(large_file_key.clone()))
        .await?;
    if uploaded.content_length != options.large_file_bytes {
        return Err(format!(
            "disk-backed upload stored {} bytes, expected {}",
            uploaded.content_length, options.large_file_bytes
        )
        .into());
    }
    client
        .delete_object(DeleteObjectRequest::new(large_file_key))
        .await?;
    drop(large_file);

    let payload_len = usize::try_from(options.payload_bytes)?;
    let payload = Bytes::from(vec![0xa5; payload_len]);

    let put_key = object_key(format!("{namespace}/single-put"))?;
    let before = rss.begin_phase();
    let started = Instant::now();
    client
        .put_object(PutObjectRequest::new(
            put_key.clone(),
            ByteStream::from_bytes(payload.clone()),
        ))
        .await?;
    let single_put = Timing::transfer(
        started.elapsed(),
        options.payload_bytes,
        rss.finish_phase(before),
    );

    let before = rss.begin_phase();
    let started = Instant::now();
    let mut output = client
        .get_object(GetObjectRequest::new(put_key.clone()))
        .await?;
    let mut downloaded = 0_u64;
    while let Some(chunk) = output.body.next().await {
        let chunk = chunk?;
        downloaded = downloaded
            .checked_add(u64::try_from(chunk.len())?)
            .ok_or("download length overflow")?;
        std::hint::black_box(&chunk);
    }
    let get = Timing::transfer(started.elapsed(), downloaded, rss.finish_phase(before));
    if downloaded != options.payload_bytes {
        return Err(format!(
            "downloaded {downloaded} bytes, expected {}",
            options.payload_bytes
        )
        .into());
    }
    client
        .delete_object(DeleteObjectRequest::new(put_key))
        .await?;

    let multipart_key = object_key(format!("{namespace}/managed-multipart"))?;
    let before = rss.begin_phase();
    let started = Instant::now();
    client
        .multipart_upload(
            ManagedMultipartUploadRequest::from_bytes(multipart_key.clone(), payload)
                .with_options(multipart_options),
        )
        .await?;
    let multipart = Timing::transfer(
        started.elapsed(),
        options.payload_bytes,
        rss.finish_phase(before),
    );
    client
        .delete_object(DeleteObjectRequest::new(multipart_key))
        .await?;

    let generated = OffsetDateTime::now_utc();
    let multipart_in_flight_budget_bytes = options.part_bytes * u64::try_from(options.concurrency)?;
    let disk_backed_peak_rss_limit_bytes = multipart_in_flight_budget_bytes
        .checked_mul(4)
        .ok_or("disk-backed RSS limit overflow")?;
    let disk_backed_observed_delta = disk_backed_multipart.peak_rss_delta_bytes;
    let disk_backed_bound_passed = disk_backed_observed_delta
        .is_some_and(|observed| observed <= disk_backed_peak_rss_limit_bytes);
    let report = Report {
        schema_version: 2,
        generated_at_utc: generated.format(&Rfc3339)?,
        generated_at_unix_seconds: u64::try_from(generated.unix_timestamp())?,
        environment: collect_environment(endpoint),
        workload: Workload {
            payload_bytes: options.payload_bytes,
            payload_pattern: "every byte is 0xa5",
            multipart_part_bytes: options.part_bytes,
            multipart_concurrency: options.concurrency,
            multipart_in_flight_budget_bytes,
            disk_backed_peak_rss_limit_bytes,
            large_file_bytes: options.large_file_bytes,
            file_generation_buffer_bytes: FILE_GENERATION_BUFFER_BYTES,
            measured_iterations_per_transfer: 1,
            warmup: "one 6-byte PUT and DELETE before measured transfers",
        },
        measurements: Measurements {
            cold_client_construction: construction,
            disk_backed_managed_multipart_upload: disk_backed_multipart,
            disk_backed_rss_bound: BoundCheck {
                observed_peak_rss_delta_bytes: disk_backed_observed_delta,
                limit_bytes: disk_backed_peak_rss_limit_bytes,
                passed: disk_backed_bound_passed,
                rationale: "four times the derived multipart in-flight byte bound",
            },
            single_put,
            streaming_get_to_sink: get,
            managed_multipart_upload: multipart,
            sampled_process_peak_rss_bytes: rss.sampled_process_peak(),
        },
        caveats: vec![
            "This is a local single-process MinIO measurement, not a comparative benchmark.",
            "Each transfer is measured once; scheduler, filesystem, loopback, CPU scaling, and container state affect results.",
            "Transfer timing includes client hashing, signing, serialization, and response handling; server startup and the warm-up request are excluded.",
            "GET is consumed incrementally without retaining the body and verifies byte count, but does not persist or hash the downloaded bytes.",
            "RSS is sampled from Linux /proc every 1 ms and is process-wide; allocator retention and prior phases make per-phase peaks non-isolated.",
            "The disk-backed phase generates its configured source with one reusable 64 KiB buffer, then includes the client's disk snapshot and bounded multipart upload in its timing.",
            "The opt-in regression check permits four times the derived multipart in-flight byte bound for part buffers, transport copies, runtime state, and allocator behavior; it is an engineering envelope, not a precise allocation model.",
            "A peak-RSS delta at one object size demonstrates this run's bounded behavior but is not a proof of asymptotic memory usage; Linux page cache is outside process RSS.",
            "The aggregate process peak uses the same RSS sampler as phase peaks; kernel VmHWM is intentionally omitted because its accounting can lag sampled VmRSS.",
            "Cold client construction means the first S3Client::new call in this process; configuration construction is excluded.",
            "The fixed repeated-byte payload is deterministic but does not model every production workload.",
        ],
    };

    write_outputs(&options, &report)?;
    println!("wrote {}", options.json_path.display());
    println!("wrote {}", options.markdown_path.display());
    if !disk_backed_bound_passed {
        return Err(format!(
            "disk-backed upload peak RSS delta {:?} exceeded the {} byte limit",
            disk_backed_observed_delta, disk_backed_peak_rss_limit_bytes
        )
        .into());
    }
    Ok(())
}

fn parse_options() -> Result<Options> {
    let payload_mib = env_u64("S3_PERF_PAYLOAD_MIB", DEFAULT_PAYLOAD_MIB)?;
    let large_file_mib = env_u64("S3_PERF_LARGE_FILE_MIB", DEFAULT_LARGE_FILE_MIB)?;
    let part_mib = env_u64("S3_PERF_PART_MIB", DEFAULT_PART_MIB)?;
    let concurrency = env_usize("S3_PERF_MULTIPART_CONCURRENCY", DEFAULT_CONCURRENCY)?;
    if !(1..=4096).contains(&payload_mib) {
        return Err("S3_PERF_PAYLOAD_MIB must be between 1 and 4096".into());
    }
    if !(5..=8192).contains(&large_file_mib) {
        return Err("S3_PERF_LARGE_FILE_MIB must be between 5 and 8192".into());
    }
    if !(5..=5120).contains(&part_mib) {
        return Err("S3_PERF_PART_MIB must be between 5 and 5120".into());
    }
    if concurrency == 0 || concurrency > 64 {
        return Err("S3_PERF_MULTIPART_CONCURRENCY must be between 1 and 64".into());
    }
    let payload_bytes = payload_mib
        .checked_mul(MIB)
        .ok_or("payload size overflow")?;
    let large_file_bytes = large_file_mib
        .checked_mul(MIB)
        .ok_or("large file size overflow")?;
    let part_bytes = part_mib.checked_mul(MIB).ok_or("part size overflow")?;
    if payload_bytes < part_bytes {
        return Err("the payload must be at least one multipart part".into());
    }
    if large_file_bytes < part_bytes {
        return Err("the large file must be at least one multipart part".into());
    }
    Ok(Options {
        payload_bytes,
        large_file_bytes,
        part_bytes,
        concurrency,
        json_path: std::env::var_os("S3_PERF_JSON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("measurements/results/minio-local.json")),
        markdown_path: std::env::var_os("S3_PERF_MARKDOWN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("measurements/results/minio-local.md")),
    })
}

fn object_key(value: String) -> Result<ObjectKey> {
    ObjectKey::new(value).map_err(Into::into)
}

fn required_env(name: &str) -> Result<String> {
    std::env::var(name)
        .map_err(|_| format!("required environment variable {name} is not set").into())
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into()),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} must be an unsigned integer").into()),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn current_rss_bytes() -> Option<u64> {
    proc_status_kib("VmRSS:").and_then(|kib| kib.checked_mul(1024))
}

fn proc_status_kib(field: &str) -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix(field)?
            .split_ascii_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn total_memory_bytes() -> Option<u64> {
    let memory = fs::read_to_string("/proc/meminfo").ok()?;
    memory.lines().find_map(|line| {
        line.strip_prefix("MemTotal:")?
            .split_ascii_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}

fn collect_environment(endpoint: String) -> Environment {
    Environment {
        git_revision: command_output("git", &["rev-parse", "HEAD"]),
        git_worktree_dirty: !command_output("git", &["status", "--porcelain"]).is_empty(),
        rustc: command_output("rustc", &["--version"]),
        cargo: command_output("cargo", &["--version"]),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
        kernel: command_output("uname", &["-srvmo"]),
        cpu_model: cpu_model(),
        logical_cpus: thread::available_parallelism().map_or(1, usize::from),
        total_memory_bytes: total_memory_bytes(),
        docker_server: command_output("docker", &["version", "--format", "{{.Server.Version}}"]),
        minio_release: std::env::var("MINIO_RELEASE").unwrap_or_else(|_| "unknown".to_owned()),
        minio_image: std::env::var("MINIO_IMAGE").unwrap_or_else(|_| "unknown".to_owned()),
        endpoint,
        build_profile: std::env::var("S3_PERF_BUILD_PROFILE")
            .unwrap_or_else(|_| "unknown".to_owned()),
    }
}

fn cpu_model() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|cpuinfo| {
            cpuinfo.lines().find_map(|line| {
                line.strip_prefix("model name")
                    .and_then(|rest| rest.split_once(':'))
                    .map(|(_, value)| value.trim().to_owned())
            })
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn command_output(command: &str, arguments: &[&str]) -> String {
    Command::new(command)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|output| !output.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn generate_deterministic_file(bytes: u64) -> Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    let buffer = [0x3c; FILE_GENERATION_BUFFER_BYTES];
    let mut remaining = bytes;
    while remaining != 0 {
        let write_length = usize::try_from(remaining.min(buffer.len() as u64))?;
        file.write_all(&buffer[..write_length])?;
        remaining -= u64::try_from(write_length)?;
    }
    file.flush()?;
    Ok(file)
}

fn write_outputs(options: &Options, report: &Report) -> Result<()> {
    ensure_parent(&options.json_path)?;
    ensure_parent(&options.markdown_path)?;
    fs::write(&options.json_path, serde_json::to_vec_pretty(report)?)?;
    fs::write(&options.markdown_path, render_markdown(report))?;
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn render_markdown(report: &Report) -> String {
    let mut output = format!(
        "# Local MinIO measurement\n\n- Generated: `{}`\n- Revision: `{}`{}\n- MinIO: `{}`\n- Image: `{}`\n- Host: `{}`\n- CPU: `{}` ({} logical CPUs)\n- Rust: `{}`\n- In-memory payload: {} MiB (`{}`)\n- Disk-backed payload: {} MiB (generated with a {} KiB buffer)\n- Multipart: {} MiB × concurrency {}\n\n| Measurement | Object size | Elapsed | Throughput | RSS before | Peak RSS | Peak RSS delta | Delta/object |\n|---|---:|---:|---:|---:|---:|---:|---:|\n",
        report.generated_at_utc,
        report.environment.git_revision,
        if report.environment.git_worktree_dirty {
            " (dirty)"
        } else {
            ""
        },
        report.environment.minio_release,
        report.environment.minio_image,
        report.environment.kernel,
        report.environment.cpu_model,
        report.environment.logical_cpus,
        report.environment.rustc,
        report.workload.payload_bytes / MIB,
        report.workload.payload_pattern,
        report.workload.large_file_bytes / MIB,
        report.workload.file_generation_buffer_bytes / 1024,
        report.workload.multipart_part_bytes / MIB,
        report.workload.multipart_concurrency,
    );
    markdown_row(
        &mut output,
        "Cold `S3Client` construction",
        &report.measurements.cold_client_construction,
    );
    markdown_row(
        &mut output,
        "Disk-backed managed multipart upload",
        &report.measurements.disk_backed_managed_multipart_upload,
    );
    markdown_row(&mut output, "Single PUT", &report.measurements.single_put);
    markdown_row(
        &mut output,
        "Streaming GET to sink",
        &report.measurements.streaming_get_to_sink,
    );
    markdown_row(
        &mut output,
        "Managed multipart upload",
        &report.measurements.managed_multipart_upload,
    );
    let bound = &report.measurements.disk_backed_rss_bound;
    output.push_str(&format!(
        "\nDisk-backed RSS bound: **{}** — observed {}, limit {} ({}).\n",
        if bound.passed { "PASS" } else { "FAIL" },
        bound
            .observed_peak_rss_delta_bytes
            .map_or_else(|| "unavailable".to_owned(), format_mib),
        format_mib(bound.limit_bytes),
        bound.rationale,
    ));
    output.push_str("\n## Method and caveats\n\n");
    for caveat in &report.caveats {
        output.push_str("- ");
        output.push_str(caveat);
        output.push('\n');
    }
    output
}

fn markdown_row(output: &mut String, name: &str, timing: &Timing) {
    let object_size = timing.bytes.map_or_else(
        || "—".to_owned(),
        |bytes| format!("{:.2} MiB", bytes as f64 / MIB as f64),
    );
    let throughput = timing
        .throughput_mib_per_second
        .map_or_else(|| "—".to_owned(), |value| format!("{value:.2} MiB/s"));
    let rss_before = timing
        .rss_before_bytes
        .map_or_else(|| "unavailable".to_owned(), format_mib);
    let peak = timing
        .peak_rss_bytes
        .map_or_else(|| "unavailable".to_owned(), format_mib);
    let delta = timing
        .peak_rss_delta_bytes
        .map_or_else(|| "unavailable".to_owned(), format_mib);
    let ratio = timing
        .peak_rss_delta_to_object_size
        .map_or_else(|| "—".to_owned(), |value| format!("{:.2}%", value * 100.0));
    output.push_str(&format!(
        "| {name} | {object_size} | {:.6} s | {throughput} | {rss_before} | {peak} | {delta} | {ratio} |\n",
        timing.elapsed_seconds
    ));
}

fn format_mib(bytes: u64) -> String {
    format!("{:.2} MiB", bytes as f64 / MIB as f64)
}
