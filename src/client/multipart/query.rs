use crate::error::S3Error;
use crate::operation::{ListMultipartUploadsRequest, ListPartsRequest, UploadId};

pub(super) fn create_query() -> Vec<(String, String)> {
    vec![("uploads".to_owned(), String::new())]
}

pub(super) fn upload_query(upload_id: &UploadId) -> Vec<(String, String)> {
    vec![("uploadId".to_owned(), upload_id.as_str().to_owned())]
}

pub(super) fn upload_part_query(part_number: u16, upload_id: &UploadId) -> Vec<(String, String)> {
    vec![
        ("partNumber".to_owned(), part_number.to_string()),
        ("uploadId".to_owned(), upload_id.as_str().to_owned()),
    ]
}

pub(super) fn list_parts_query(request: &ListPartsRequest) -> Vec<(String, String)> {
    let mut query = vec![
        ("max-parts".to_owned(), request.max_parts.get().to_string()),
        (
            "uploadId".to_owned(),
            request.upload_id().as_str().to_owned(),
        ),
    ];
    if let Some(marker) = request.part_number_marker {
        query.push(("part-number-marker".to_owned(), marker.get().to_string()));
    }
    query
}

pub(super) fn list_query(
    request: &ListMultipartUploadsRequest,
) -> Result<Vec<(String, String)>, S3Error> {
    if request.upload_id_marker.is_some() && request.key_marker.is_none() {
        return Err(S3Error::configuration(
            "an upload-ID marker requires a key marker",
        ));
    }
    let mut query = create_query();
    if let Some(delimiter) = &request.delimiter {
        query.push(("delimiter".to_owned(), delimiter.clone()));
    }
    if let Some(key_marker) = &request.key_marker {
        query.push(("key-marker".to_owned(), key_marker.clone()));
    }
    query.push((
        "max-uploads".to_owned(),
        request.max_uploads.get().to_string(),
    ));
    if let Some(prefix) = &request.prefix {
        query.push(("prefix".to_owned(), prefix.clone()));
    }
    if let Some(upload_id_marker) = &request.upload_id_marker {
        query.push((
            "upload-id-marker".to_owned(),
            upload_id_marker.as_str().to_owned(),
        ));
    }
    Ok(query)
}
