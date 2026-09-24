use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocFormat {
    Hwp5,
    Hwpx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    Input,
    ZipEntries,
    ZipUncompressed,
    Output,
    Time,
    Memory,
    Decompress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerFailureReason {
    MissingExecutable,
    Spawn,
    Wait,
    ChildCrash,
    InvalidChildJson,
    LimitApply,
    UnsupportedPlatform,
    InvalidLimits,
    SlotPoison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    Encrypted,
    Distribution,
    Drm,
    ExtensionMagicMismatch,
    Hwp3,
    Hml,
    UnknownFormat,
    EmptyFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExtractStatus {
    Ok {
        text: String,
        format: DocFormat,
        warnings: Vec<String>,
    },
    Empty {
        format: DocFormat,
        warnings: Vec<String>,
    },
    Partial {
        text: String,
        format: DocFormat,
        warnings: Vec<String>,
    },
    Unsupported {
        reason: UnsupportedReason,
        detail: String,
    },
    Corrupt {
        detail: String,
    },
    ResourceLimit {
        kind: LimitKind,
        detail: String,
    },
    WorkerFailure {
        reason: WorkerFailureReason,
        detail: String,
    },
}

impl ExtractStatus {
    pub fn text(&self) -> &str {
        match self {
            Self::Ok { text, .. } | Self::Partial { text, .. } => text,
            _ => "",
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(
            self,
            Self::Ok { .. } | Self::Empty { .. } | Self::Partial { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractReport {
    pub extractor: String,
    pub rhwp_rev: String,
    pub outcome: ExtractStatus,
    pub used_preview_stream: bool,
    /// Set when a child process was spawned. In-process extracts leave this `None`.
    #[serde(default)]
    pub child_pid: Option<u32>,
}

impl ExtractReport {
    pub fn new(outcome: ExtractStatus) -> Self {
        Self {
            extractor: "rhwp-native".to_string(),
            rhwp_rev: crate::RHWP_REV.to_string(),
            outcome,
            used_preview_stream: false,
            child_pid: None,
        }
    }

    pub fn with_child_pid(mut self, pid: u32) -> Self {
        self.child_pid = Some(pid);
        self
    }
}
