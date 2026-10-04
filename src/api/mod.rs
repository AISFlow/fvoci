pub mod collections_dto;
pub mod documents_dto;
pub mod dto;
pub mod native_archive;
pub mod personal_input_dto;
pub mod personal_transfer;
pub mod task_timer;
pub mod tasks_dto;

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
pub mod openapi;

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
pub mod openapi_identity;

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
pub mod openapi_documents;

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
pub mod openapi_tasks;

pub mod zotero_dto;

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
pub mod openapi_zotero;
