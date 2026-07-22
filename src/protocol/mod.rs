//! Bounded S3 XML serialization and parsing.

mod delete;
mod error;
mod listing;
mod multipart;
mod xml;

pub(crate) use delete::{parse_delete_objects, serialize_delete_objects};
pub(crate) use error::{ParsedS3Error, parse_s3_error};
pub(crate) use listing::parse_list_objects_v2;
pub(crate) use multipart::{
    CompleteMultipartResponse, CopyObjectResponse, UploadPartCopyResponse,
    parse_complete_multipart_upload, parse_copy_object, parse_create_multipart_upload,
    parse_list_multipart_uploads, parse_list_parts, parse_upload_part_copy,
    serialize_complete_multipart_upload,
};
pub(crate) use xml::ProtocolError;
