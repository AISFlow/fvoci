use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitKind {
    Input,
    Output,
    Frame,
    Ops,
    Time,
    Memory,
    Stack,
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
    SessionDead,
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedReason {
    EncodingV2,
    UnknownOp,
    SnapshotRestore,
}

/// Engine result. `applied` means this child's in-memory Doc integrated bytes.
/// It is **not** a durable/authoritative mutation: the parent broadcasts only
/// after a DB commit, and never treats Yrs undo as a DB rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EngineStatus {
    Ok {
        applied: bool,
        pending: bool,
        durable: bool,
        skip_gc: bool,
        offset_kind: String,
        encoding: u8,
        fragment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update_b64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state_vector_b64: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        xml_string: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        xml_len: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        yrs: Option<String>,
    },
    Malformed {
        detail: String,
    },
    Unsupported {
        reason: UnsupportedReason,
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

impl EngineStatus {
    pub fn ping_ok() -> Self {
        Self::Ok {
            applied: false,
            pending: false,
            durable: false,
            skip_gc: true,
            offset_kind: "utf16".into(),
            encoding: 1,
            fragment: crate::FRAGMENT.into(),
            update_b64: None,
            state_vector_b64: None,
            xml_string: None,
            xml_len: None,
            yrs: Some(crate::YRS_VERSION.into()),
        }
    }

    pub fn is_applied_ok(&self) -> bool {
        matches!(self, Self::Ok { applied: true, .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineReport {
    pub engine: String,
    pub yrs: String,
    pub outcome: EngineStatus,
    #[serde(default)]
    pub child_pid: Option<u32>,
}

impl EngineReport {
    pub fn new(outcome: EngineStatus) -> Self {
        Self {
            engine: "collab-engine".into(),
            yrs: crate::YRS_VERSION.into(),
            outcome,
            child_pid: None,
        }
    }

    pub fn with_child_pid(mut self, pid: u32) -> Self {
        self.child_pid = Some(pid);
        self
    }
}
