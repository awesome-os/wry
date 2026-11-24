// Copyright 2026 Awesome-OS Contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Sectium Host Device Boundary backend for WRY (Layer 3 `sections/sectium-device/`).
//!
//! Routes webview rendering and DevTools/IDE IPC over the dual-channel NDJSON
//! host pipe (`\\.\pipe\sectium-dev` on Windows, `/tmp/sectium-dev.sock` on POSIX)
//! using `B8G_ABI_MAGIC_V3` (`0x33564742`) with **zero Microsoft WebView2** and
//! **zero local loopback HTTP servers**.

use std::io::{self, Write};

/// Canonical `B8G_ABI_MAGIC_V3` magic constant (`"BGV3"` in little-endian).
pub const SECTIUM_DEVICE_ABI_MAGIC_V3: u32 = 0x33564742;

/// Default Windows named pipe path for the Sectium Host Device Boundary.
pub const DEFAULT_WINDOWS_PIPE_PATH: &str = r"\\.\pipe\sectium-dev";

/// Default POSIX Unix domain socket path for the Sectium Host Device Boundary.
pub const DEFAULT_POSIX_SOCKET_PATH: &str = "/tmp/sectium-dev.sock";

/// Returns the platform-canonical Sectium host device pipe/socket path.
///
/// - Windows: `\\.\pipe\sectium-dev`
/// - POSIX (Linux / macOS / BSD): `/tmp/sectium-dev.sock`
#[must_use]
pub const fn default_sectium_pipe_path() -> &'static str {
  #[cfg(windows)]
  {
    DEFAULT_WINDOWS_PIPE_PATH
  }
  #[cfg(not(windows))]
  {
    DEFAULT_POSIX_SOCKET_PATH
  }
}

/// Dual opaque interchangeable channels on the Sectium Host Device Boundary.
///
/// - [`SectiumChannel::Display`]: Pure art / viewport navigation sink.
/// - [`SectiumChannel::Command`]: `ttyS0` control, script evaluation, and DevTools/IDE IPC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectiumChannel {
  /// Pure-art display sink (`"display"`).
  Display,
  /// Control, script evaluation, and cross-tab DevTools/IDE IPC (`"command"`).
  Command,
}

impl SectiumChannel {
  /// Returns the wire-format channel identifier (`"display"` or `"command"`).
  #[must_use]
  pub const fn as_str(self) -> &'static str {
    match self {
      Self::Display => "display",
      Self::Command => "command",
    }
  }
}

/// Single NDJSON wire frame exchanged over the Sectium Host Device Boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectiumNdjsonFrame {
  /// Target channel (`display` for pure art, `command` for IPC/DevTools/scripts).
  pub channel: SectiumChannel,
  /// Operation verb (e.g., `"load_url"`, `"evaluate_script"`, `"ipc"`).
  pub op: String,
  /// Optional URL payload (used by `load_url` on `display`).
  pub url: Option<String>,
  /// Optional script or IPC payload (used on `command`).
  pub script: Option<String>,
  /// `B8G_ABI_MAGIC_V3` (`0x33564742`) protocol discriminator.
  pub abi_magic: u32,
}

impl SectiumNdjsonFrame {
  /// Constructs a new frame stamped with [`SECTIUM_DEVICE_ABI_MAGIC_V3`].
  #[must_use]
  pub fn new(
    channel: SectiumChannel,
    op: impl Into<String>,
    url: Option<String>,
    script: Option<String>,
  ) -> Self {
    Self {
      channel,
      op: op.into(),
      url,
      script,
      abi_magic: SECTIUM_DEVICE_ABI_MAGIC_V3,
    }
  }

  /// Encodes this frame as a single newline-delimited JSON (NDJSON) line.
  #[must_use]
  pub fn encode_ndjson(&self) -> String {
    let mut out = String::with_capacity(128);
    out.push('{');
    out.push_str("\"channel\":\"");
    out.push_str(self.channel.as_str());
    out.push_str("\",\"op\":\"");
    push_json_escaped(&mut out, &self.op);
    out.push('"');

    if let Some(ref url) = self.url {
      out.push_str(",\"url\":\"");
      push_json_escaped(&mut out, url);
      out.push('"');
    } else {
      out.push_str(",\"url\":null");
    }

    if let Some(ref script) = self.script {
      out.push_str(",\"script\":\"");
      push_json_escaped(&mut out, script);
      out.push('"');
    } else {
      out.push_str(",\"script\":null");
    }

    out.push_str(",\"abi_magic\":");
    out.push_str(&self.abi_magic.to_string());
    out.push_str("}\n");
    out
  }
}

/// Zero-WebView2 WebView facade speaking NDJSON over the Sectium Host Device pipe.
#[derive(Clone, Debug)]
pub struct SectiumWebView {
  pipe_path: String,
  outbound_frames: Vec<SectiumNdjsonFrame>,
}

impl SectiumWebView {
  /// Creates a new `SectiumWebView` bound to the given host pipe/socket path.
  #[must_use]
  pub fn new(pipe_path: impl Into<String>) -> Self {
    Self {
      pipe_path: pipe_path.into(),
      outbound_frames: Vec::new(),
    }
  }

  /// Creates a new `SectiumWebView` bound to [`default_sectium_pipe_path()`].
  #[must_use]
  pub fn with_default_pipe() -> Self {
    Self::new(default_sectium_pipe_path())
  }

  /// Returns the configured host pipe or Unix domain socket path.
  #[must_use]
  pub fn pipe_path(&self) -> &str {
    &self.pipe_path
  }

  /// Returns all frames recorded by this `SectiumWebView` instance.
  #[must_use]
  pub fn outbound_frames(&self) -> &[SectiumNdjsonFrame] {
    &self.outbound_frames
  }

  /// Encodes a [`SectiumNdjsonFrame`] into a single NDJSON wire line.
  #[must_use]
  pub fn encode_ndjson_frame(frame: &SectiumNdjsonFrame) -> String {
    frame.encode_ndjson()
  }

  /// Navigates the pure-art `display` sink to `url` and returns the encoded NDJSON frame.
  pub fn load_url(&mut self, url: &str) -> String {
    let frame = SectiumNdjsonFrame::new(
      SectiumChannel::Display,
      "load_url",
      Some(url.to_owned()),
      None,
    );
    let encoded = Self::encode_ndjson_frame(&frame);
    self.outbound_frames.push(frame);
    encoded
  }

  /// Evaluates JavaScript over the `command` channel (keeping `display` pure art)
  /// and returns the encoded NDJSON frame.
  pub fn evaluate_script(&mut self, script: &str) -> String {
    let frame = SectiumNdjsonFrame::new(
      SectiumChannel::Command,
      "evaluate_script",
      None,
      Some(script.to_owned()),
    );
    let encoded = Self::encode_ndjson_frame(&frame);
    self.outbound_frames.push(frame);
    encoded
  }

  /// Sends a DevTools/IDE IPC message over the `command` channel and returns the
  /// encoded NDJSON frame.
  pub fn send_command_ipc(&mut self, message: &str) -> String {
    let frame = SectiumNdjsonFrame::new(
      SectiumChannel::Command,
      "ipc",
      None,
      Some(message.to_owned()),
    );
    let encoded = Self::encode_ndjson_frame(&frame);
    self.outbound_frames.push(frame);
    encoded
  }

  /// Writes an encoded NDJSON frame directly to the configured host pipe/socket.
  ///
  /// Uses OS named pipes on Windows and Unix domain sockets on POSIX — zero HTTP.
  pub fn write_frame_to_pipe(&self, ndjson_line: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
      let mut pipe = std::fs::OpenOptions::new()
        .write(true)
        .open(&self.pipe_path)?;
      pipe.write_all(ndjson_line.as_bytes())?;
      pipe.flush()
    }
    #[cfg(not(windows))]
    {
      let mut stream = std::os::unix::net::UnixStream::connect(&self.pipe_path)?;
      stream.write_all(ndjson_line.as_bytes())?;
      stream.flush()
    }
  }
}

fn push_json_escaped(out: &mut String, input: &str) {
  for ch in input.chars() {
    match ch {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      c if (c as u32) < 0x20 => {
        out.push_str(&format!("\\u{:04x}", c as u32));
      }
      c => out.push(c),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_default_sectium_pipe_path() {
    let path = default_sectium_pipe_path();
    #[cfg(windows)]
    assert_eq!(path, r"\\.\pipe\sectium-dev");
    #[cfg(not(windows))]
    assert_eq!(path, "/tmp/sectium-dev.sock");

    // Strictly enforce host-pipe-ipc-mandate: zero loopback HTTP.
    assert!(!path.contains("http://"));
    assert!(!path.contains("https://"));
    assert!(!path.contains("127.0.0.1"));
  }

  #[test]
  fn test_abi_magic_v3_constant() {
    assert_eq!(SECTIUM_DEVICE_ABI_MAGIC_V3, 0x33564742);
    assert_eq!(&SECTIUM_DEVICE_ABI_MAGIC_V3.to_le_bytes(), b"BGV3");
  }

  #[test]
  fn test_dual_channel_ndjson_encoding_display_vs_command() {
    let mut wv = SectiumWebView::new(default_sectium_pipe_path());
    assert_eq!(wv.pipe_path(), default_sectium_pipe_path());

    // 1. Display channel: pure art navigation
    let display_line = wv.load_url("https://awesome-os.com/canvas");
    assert!(display_line.ends_with('\n'));
    assert_eq!(display_line.matches('\n').count(), 1);
    assert!(display_line.contains("\"channel\":\"display\""));
    assert!(display_line.contains("\"op\":\"load_url\""));
    assert!(display_line.contains("\"url\":\"https://awesome-os.com/canvas\""));
    assert!(display_line.contains("\"script\":null"));
    assert!(display_line.contains(&format!("\"abi_magic\":{}", SECTIUM_DEVICE_ABI_MAGIC_V3)));

    // 2. Command channel: script evaluation
    let eval_line = wv.evaluate_script("window.__SECTIUM__ = \"ok\";");
    assert!(eval_line.ends_with('\n'));
    assert_eq!(eval_line.matches('\n').count(), 1);
    assert!(eval_line.contains("\"channel\":\"command\""));
    assert!(eval_line.contains("\"op\":\"evaluate_script\""));
    assert!(eval_line.contains("\"url\":null"));
    assert!(eval_line.contains("\"script\":\"window.__SECTIUM__ = \\\"ok\\\";\""));
    assert!(eval_line.contains(&format!("\"abi_magic\":{}", SECTIUM_DEVICE_ABI_MAGIC_V3)));

    // 3. Command channel: DevTools/IDE IPC
    let ipc_line = wv.send_command_ipc("{\"method\":\"Runtime.enable\"}");
    assert!(ipc_line.ends_with('\n'));
    assert_eq!(ipc_line.matches('\n').count(), 1);
    assert!(ipc_line.contains("\"channel\":\"command\""));
    assert!(ipc_line.contains("\"op\":\"ipc\""));
    assert!(ipc_line.contains("\"script\":\"{\\\"method\\\":\\\"Runtime.enable\\\"}\""));
    assert!(ipc_line.contains(&format!("\"abi_magic\":{}", SECTIUM_DEVICE_ABI_MAGIC_V3)));

    // Verify recorded frames preserve dual-channel separation.
    assert_eq!(wv.outbound_frames().len(), 3);
    assert_eq!(wv.outbound_frames()[0].channel, SectiumChannel::Display);
    assert_eq!(wv.outbound_frames()[1].channel, SectiumChannel::Command);
    assert_eq!(wv.outbound_frames()[2].channel, SectiumChannel::Command);
  }

  #[test]
  fn test_ndjson_escaping_preserves_single_line() {
    let frame = SectiumNdjsonFrame::new(
      SectiumChannel::Command,
      "evaluate_script",
      None,
      Some("line1\nline2\r\t\"quoted\"\\path".to_owned()),
    );
    let encoded = SectiumWebView::encode_ndjson_frame(&frame);
    assert_eq!(encoded.matches('\n').count(), 1);
    assert!(encoded.ends_with('\n'));
    assert!(encoded.contains("line1\\nline2\\r\\t\\\"quoted\\\"\\\\path"));
  }

  #[test]
  fn test_zero_webview2_dependency_in_sectium_device() {
    let src = include_str!("sectium_device.rs");
    let non_test_code: String = src
      .split("#[cfg(test)]")
      .next()
      .unwrap_or(src)
      .lines()
      .filter(|line| !line.trim_start().starts_with("//"))
      .collect::<Vec<_>>()
      .join("\n");

    assert!(
      !non_test_code.contains("webview2_com"),
      "sectium_device must never depend on webview2_com"
    );
    assert!(
      !non_test_code.contains("ICoreWebView2"),
      "sectium_device must never reference ICoreWebView2"
    );
    assert!(
      !non_test_code.contains("127.0.0.1"),
      "sectium_device must never bind or dial loopback HTTP"
    );
  }
}
