//! Error codes. Every reply with `"ok": false` and every `error` event carries `code`, `msg`, `source`.

/// (code, source, message, what to do). The README table is generated from this (`mc-server errors`).
pub const TABLE: &[(&str, &str, &str, &str)] = &[
    // session / network
    ("E100", "net", "Unknown command", "Check the command name; client and server versions should match"),
    ("E101", "net", "Bad request", "A value is missing or out of range: see the detail after the message"),
    ("E102", "net", "Read-only: viewers cannot control the microscope", "Connect as operator"),
    ("E103", "net", "Another operator holds the microscope", "Wait until it disconnects or releases control"),
    ("E104", "net", "Say hello first", "Client bug: the first message must be hello"),
    ("E105", "net", "Operator silent too long: stage stopped, laser off", "Check the network; the client reconnects by itself"),
    ("E106", "net", "Unknown session for the data connection", "Open the control connection first; the client does this by itself"),
    ("E107", "net", "Frame no longer in the ring", "Ask sooner, or start the server with a larger --ring"),
    ("E108", "net", "Too many requests", "Viewers may fetch one full frame per second"),
    ("E109", "client", "No reply from the server in time", "Check that the server runs and the network is up"),
    ("E110", "client", "Connection to the server lost", "The client reconnects by itself; check the state before repeating a relative move"),
    ("E111", "net", "Address not allowed", "Add the client's address or network to --allow on the server"),
    ("E112", "net", "Wrong or missing token", "Give the server's token: Microscope(token=...) or MCONTROL_TOKEN"),
    ("E113", "net", "Control not allowed from this address", "Connect as viewer, or add the address to --operators on the server"),
    ("E120", "net", "Cannot listen on this address or port", "Another program uses the port, or the --bind address is not on this PC"),
    ("E121", "net", "Open beyond the local network without a token", "Give --token, or keep --allow to local networks"),
    // stage
    ("E201", "stage", "Outside the soft limits", "Stay inside the stage range (mc-server --help shows the limits)"),
    ("E202", "stage", "Limit switch reached", "Move back the other way; check the zero"),
    ("E203", "stage", "Move stopped", "Someone sent stop, or the operator was lost; move again"),
    ("E205", "stage", "Z move too large for one command", "Split the Z move into smaller steps"),
    // controller
    ("E300", "controller", "Controller not connected", "Plug in the USB cable, or give --port COMx"),
    ("E301", "controller", "No answer from the controller", "Close the Optosky software, replug the USB, power-cycle the controller"),
    ("E302", "controller", "Bad reply from the controller", "Usually noise on the line: repeat; replug if it stays"),
    ("E303", "controller", "Checksum error", "Usually noise on the line: repeat; replug if it stays"),
    ("E304", "controller", "Reply to another command", "Repeat; another program may be using the port"),
    ("E305", "controller", "Command not allowed", "Only the known Optosky commands are sent to the controller"),
    ("E306", "controller", "Controller reports a failure", "Check the motors and the cable; power-cycle the controller"),
    ("E307", "controller", "Serial port error", "Replug the USB; check the port in Device Manager"),
    ("E308", "controller", "Laser not allowed (start mc-server with --allow-laser)", "Start the server with --allow-laser"),
    ("E309", "controller", "Laser power above the maximum", "Ask for no more than the maximum the controller reports"),
    // camera
    ("E500", "camera", "Camera not available", "Wait a few seconds after start; see the CAM lines in the server log"),
    ("E501", "camera", "No camera found", "Plug the camera into USB 3; check it in MindVision's demo program"),
    ("E502", "camera", "MindVision camera SDK not found: install it from mindvision.com.cn (Downloads -> SDK)", "Install the MindVision SDK, or give --sdk DIR"),
    ("E503", "camera", "Camera SDK error", "Replug the camera; restart the server"),
    ("E504", "camera", "Camera used by another program (close the Optosky software)", "Close the Optosky software and MindVision's demo"),
    ("E505", "camera", "No frame from the camera", "Check the cable and the exposure; the worker restarts by itself"),
    ("E506", "camera", "Camera worker stopped", "It restarts by itself; if it repeats, replug the camera"),
    // guard
    ("E600", "guard", "Server not answering: guard stopped the stage and switched the laser off", "Start mc-server again; look at the log for why it hung"),
];

/// Markdown table of all codes (for the README).
pub fn markdown() -> String {
    let mut s = String::from("| Code | Where | Message | What to do |\n|---|---|---|---|\n");
    for (c, src, m, h) in TABLE {
        s += &format!("| {c} | {src} | {m} | {h} |\n");
    }
    s
}

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
