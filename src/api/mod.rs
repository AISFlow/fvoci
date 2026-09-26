pub mod collections_dto;
pub mod documents_dto;
pub mod dto;
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
pub mod openapi_tasks;
