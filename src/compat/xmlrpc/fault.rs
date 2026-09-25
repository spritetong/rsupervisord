// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
// https://github.com/spritetong/rsupervisord
//
// Licensed under the Mozilla Public License 2.0.
// SPDX-License-Identifier: MPL-2.0

use crate::error::ProgramError;

/// Supervisor standard XML-RPC Fault Codes.
/// Defined in `supervisor/xmlrpc.py::Faults`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum FaultCode {
    UnknownMethod = 1,
    IncorrectParameters = 2,
    BadArguments = 3,
    SignatureUnsupported = 4,
    ShutdownState = 6,
    BadName = 10,
    BadSignal = 11,
    NoFile = 20,
    NotExecutable = 21,
    Failed = 30,
    AbnormalTermination = 40,
    SpawnError = 50,
    AlreadyStarted = 60,
    NotRunning = 70,
    Success = 80,
    AlreadyAdded = 90,
    StillRunning = 91,
    CantReread = 92,
}

impl FaultCode {
    pub const fn code(self) -> i32 {
        self as i32
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::UnknownMethod => "UNKNOWN_METHOD",
            Self::IncorrectParameters => "INCORRECT_PARAMETERS",
            Self::BadArguments => "BAD_ARGUMENTS",
            Self::SignatureUnsupported => "SIGNATURE_UNSUPPORTED",
            Self::ShutdownState => "SHUTDOWN_STATE",
            Self::BadName => "BAD_NAME",
            Self::BadSignal => "BAD_SIGNAL",
            Self::NoFile => "NO_FILE",
            Self::NotExecutable => "NOT_EXECUTABLE",
            Self::Failed => "FAILED",
            Self::AbnormalTermination => "ABNORMAL_TERMINATION",
            Self::SpawnError => "SPAWN_ERROR",
            Self::AlreadyStarted => "ALREADY_STARTED",
            Self::NotRunning => "NOT_RUNNING",
            Self::Success => "SUCCESS",
            Self::AlreadyAdded => "ALREADY_ADDED",
            Self::StillRunning => "STILL_RUNNING",
            Self::CantReread => "CANT_REREAD",
        }
    }
}

/// An XML-RPC Fault response representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    pub code: i32,
    pub message: String,
}

impl Fault {
    pub fn new(code: FaultCode, message: impl Into<String>) -> Self {
        Self {
            code: code.code(),
            message: message.into(),
        }
    }

    pub fn unknown_method(name: &str) -> Self {
        Self::new(
            FaultCode::UnknownMethod,
            format!(
                "{}: method '{}' not found",
                FaultCode::UnknownMethod.name(),
                name
            ),
        )
    }

    pub fn incorrect_params(detail: &str) -> Self {
        Self::new(
            FaultCode::IncorrectParameters,
            format!("{}: {}", FaultCode::IncorrectParameters.name(), detail),
        )
    }

    pub fn bad_name(name: &str) -> Self {
        Self::new(
            FaultCode::BadName,
            format!("{}: {}", FaultCode::BadName.name(), name),
        )
    }

    pub fn bad_signal(sig: &str) -> Self {
        Self::new(
            FaultCode::BadSignal,
            format!("{}: {}", FaultCode::BadSignal.name(), sig),
        )
    }

    pub fn already_started(name: &str) -> Self {
        Self::new(
            FaultCode::AlreadyStarted,
            format!("{}: {}", FaultCode::AlreadyStarted.name(), name),
        )
    }

    pub fn not_running(name: &str) -> Self {
        Self::new(
            FaultCode::NotRunning,
            format!("{}: {}", FaultCode::NotRunning.name(), name),
        )
    }

    pub fn shutdown_state() -> Self {
        Self::new(
            FaultCode::ShutdownState,
            FaultCode::ShutdownState.name().to_string(),
        )
    }

    pub fn signature_unsupported(name: &str) -> Self {
        Self::new(
            FaultCode::SignatureUnsupported,
            format!("{}: {}", FaultCode::SignatureUnsupported.name(), name),
        )
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        Self::new(FaultCode::Failed, msg)
    }

    pub fn bad_arguments(detail: impl Into<String>) -> Self {
        Self::new(FaultCode::BadArguments, detail)
    }

    pub fn no_file(detail: impl Into<String>) -> Self {
        Self::new(FaultCode::NoFile, detail)
    }

    pub fn cant_reread(detail: impl Into<String>) -> Self {
        Self::new(FaultCode::CantReread, detail)
    }

    pub fn already_added(name: &str) -> Self {
        Self::new(
            FaultCode::AlreadyAdded,
            format!("{}: {}", FaultCode::AlreadyAdded.name(), name),
        )
    }

    pub fn still_running(name: &str) -> Self {
        Self::new(
            FaultCode::StillRunning,
            format!("{}: {}", FaultCode::StillRunning.name(), name),
        )
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fault {}: {}", self.code, self.message)
    }
}

impl std::error::Error for Fault {}

impl From<ProgramError> for Fault {
    fn from(err: ProgramError) -> Self {
        match err {
            ProgramError::NotFound { name } => Fault::bad_name(&name),
            ProgramError::AlreadyRunning { name, .. } => Fault::already_started(&name),
            ProgramError::NotRunning { name } => Fault::not_running(&name),
            ProgramError::ShuttingDown { .. } => Fault::shutdown_state(),
            ProgramError::Timeout { .. } => {
                Fault::new(FaultCode::AbnormalTermination, "ABNORMAL_TERMINATION")
            }
            ProgramError::StartFailed { name, source } => {
                let reason = source.to_string();
                let reason_lower = reason.to_lowercase();
                if reason_lower.contains("not found")
                    || reason_lower.contains("no such file")
                    || reason_lower.contains("cannot find")
                {
                    Fault::new(
                        FaultCode::SpawnError,
                        format!("SPAWN_ERROR: {} ({})", name, reason),
                    )
                } else if reason_lower.contains("permission denied")
                    || reason_lower.contains("access is denied")
                {
                    Fault::new(
                        FaultCode::NotExecutable,
                        format!("NOT_EXECUTABLE: {} ({})", name, reason),
                    )
                } else {
                    Fault::new(
                        FaultCode::SpawnError,
                        format!("SPAWN_ERROR: {} ({})", name, reason),
                    )
                }
            }
            ProgramError::StdinWriteTimeout { .. } => {
                Fault::new(FaultCode::NoFile, "NO_FILE: stdin pipe write timed out")
            }
            ProgramError::ConfigError(msg) => Fault::cant_reread(msg),
            ProgramError::ReadLogFailed { name: _, error } => {
                let error_lower = error.to_lowercase();
                if error_lower.contains("not exist") || error_lower.contains("no such file") {
                    Fault::no_file(error)
                } else if error_lower.contains("negative") || error_lower.contains("bad arguments")
                {
                    Fault::bad_arguments(error)
                } else {
                    Fault::failed(error)
                }
            }
            _ => Fault::failed(err.to_string()),
        }
    }
}
