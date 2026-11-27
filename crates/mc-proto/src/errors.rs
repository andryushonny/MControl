//! Error codes. Every reply with `"ok": false` and every `error` event carries `code`, `msg`, `source`.

pub const TABLE: &[(&str, &str, &str)] = &[
    // session / network
    ("E100", "net", "Unknown command"),
    ("E101", "net", "Bad request"),
    ("E102", "net", "Read-only: viewers cannot control the microscope"),
    ("E103", "net", "Another operator holds the microscope"),
    ("E104", "net", "Say hello first"),
    ("E105", "net", "Operator silent too long: stage stopped, laser off"),
    ("E106", "net", "Unknown session for the data connection"),
    ("E107", "net", "Frame no longer in the ring"),
    ("E108", "net", "Too many requests"),
    ("E109", "net", "No reply from the server in time"),
    ("E110", "net", "Connection to the server lost"),
    // stage
    ("E200", "stage", "Stage busy"),
    ("E201", "stage", "Outside the soft limits"),
    ("E202", "stage", "Limit switch reached"),
    ("E203", "stage", "Move stopped"),
    ("E204", "stage", "Position uncertain: check the stage, then confirm"),
    ("E205", "stage", "Z move too large for one command"),
    // controller
    ("E300", "controller", "Controller not connected"),
    ("E301", "controller", "No answer from the controller"),
    ("E302", "controller", "Bad reply from the controller"),
    ("E303", "controller", "Checksum error"),
    ("E304", "controller", "Reply to another command"),
    ("E305", "controller", "Command not allowed"),
    ("E306", "controller", "Controller reports a failure"),
    ("E307", "controller", "Serial port error"),
    ("E308", "controller", "Laser not allowed (start mc-server with --allow-laser)"),
    // camera
    ("E500", "camera", "Camera not available"),
    ("E501", "camera", "No camera found"),
    ("E502", "camera", "MindVision camera SDK not found: install it from mindvision.com.cn (Downloads -> SDK)"),
    ("E503", "camera", "Camera SDK error"),
    ("E504", "camera", "Camera used by another program (close the Optosky software)"),
    ("E505", "camera", "No frame from the camera"),
    ("E506", "camera", "Camera worker stopped"),
    // guard
    ("E600", "guard", "Server not answering: guard stopped the stage and switched the laser off"),
];

pub fn text(code: &str) -> &'static str {
    TABLE.iter().find(|t| t.0 == code).map(|t| t.2).unwrap_or("Error")
}

pub fn source(code: &str) -> &'static str {
    TABLE.iter().find(|t| t.0 == code).map(|t| t.1).unwrap_or("server")
}

/// An error with a code, ready to go into a reply.
#[derive(Debug, Clone)]
pub struct McError {
    pub code: &'static str,
    pub detail: String,
}

impl McError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        McError { code, detail: detail.into() }
    }

    pub fn json(&self) -> serde_json::Value {
        let msg = if self.detail.is_empty() {
            text(self.code).to_string()
        } else {
            format!("{}: {}", text(self.code), self.detail)
        };
        serde_json::json!({"code": self.code, "msg": msg, "source": source(self.code)})
    }
}

impl std::fmt::Display for McError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.code, self.json()["msg"].as_str().unwrap_or(""))
    }
}

impl std::error::Error for McError {}

pub type McResult<T> = Result<T, McError>;
