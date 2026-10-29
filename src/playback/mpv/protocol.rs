//! mpv JSON IPC wire types.
//!
//! Requests are single-line JSON objects `{"command":[...],"request_id":N}`
//! terminated by `\n`. Paths travel only as JSON strings, so quotes,
//! backslashes, newlines and leading dashes need no special handling.
//! Incoming lines are either replies (carrying `request_id`) or events
//! (carrying `event`). Unknown events and fields are ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::playback::StreamUrl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekMode {
    Relative,
    Absolute,
}

/// The subset of mpv commands Coda uses.
#[derive(Debug, Clone, PartialEq)]
pub enum MpvCommand {
    /// `["loadfile", path, "replace"]`. Only the two-argument form is used:
    /// later positional arguments changed meaning in mpv 0.38.
    LoadFile {
        path: String,
    },
    /// `["loadfile", url, "replace"]` for a signed stream URL. Kept apart
    /// from `LoadFile` so the URL stays redacted everywhere but the encoder.
    LoadStream {
        url: StreamUrl,
    },
    SetProperty {
        name: &'static str,
        value: Value,
    },
    GetProperty {
        name: &'static str,
    },
    ObserveProperty {
        id: u64,
        name: &'static str,
    },
    Seek {
        seconds: f64,
        mode: SeekMode,
    },
    Stop,
    Quit,
}

impl MpvCommand {
    fn args(&self) -> Vec<Value> {
        match self {
            Self::LoadFile { path } => {
                vec!["loadfile".into(), path.as_str().into(), "replace".into()]
            }
            Self::LoadStream { url } => {
                vec!["loadfile".into(), url.expose().into(), "replace".into()]
            }
            Self::SetProperty { name, value } => {
                vec!["set_property".into(), (*name).into(), value.clone()]
            }
            Self::GetProperty { name } => vec!["get_property".into(), (*name).into()],
            Self::ObserveProperty { id, name } => {
                vec!["observe_property".into(), (*id).into(), (*name).into()]
            }
            Self::Seek { seconds, mode } => {
                let mode = match mode {
                    SeekMode::Relative => "relative",
                    SeekMode::Absolute => "absolute",
                };
                vec!["seek".into(), (*seconds).into(), mode.into()]
            }
            Self::Stop => vec!["stop".into()],
            Self::Quit => vec!["quit".into()],
        }
    }

    /// `true` if the encoded request carries a secret (a signed URL), so
    /// the raw line must not be logged even at `trace`.
    pub fn is_sensitive(&self) -> bool {
        matches!(self, Self::LoadStream { .. })
    }

    /// Short description for logs. Never includes the load target.
    pub fn describe(&self) -> String {
        match self {
            Self::LoadFile { .. } | Self::LoadStream { .. } => "loadfile".to_owned(),
            Self::SetProperty { name, .. } => format!("set_property {name}"),
            Self::GetProperty { name } => format!("get_property {name}"),
            Self::ObserveProperty { name, .. } => format!("observe_property {name}"),
            Self::Seek { .. } => "seek".to_owned(),
            Self::Stop => "stop".to_owned(),
            Self::Quit => "quit".to_owned(),
        }
    }
}

#[derive(Serialize)]
struct Request {
    command: Vec<Value>,
    request_id: u64,
}

/// Encodes a request as one JSON line, including the trailing newline.
pub fn encode_request(request_id: u64, command: &MpvCommand) -> String {
    let request = Request {
        command: command.args(),
        request_id,
    };
    // Serializing a Vec<Value> and an integer cannot fail.
    let mut line = serde_json::to_string(&request).expect("request serializes");
    line.push('\n');
    line
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndFileReason {
    Eof,
    Stop,
    Quit,
    Error,
    Redirect,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MpvEvent {
    StartFile {
        entry_id: Option<i64>,
    },
    /// The file started by the preceding `start-file` is open and
    /// decodable. mpv does not include an entry id.
    FileLoaded,
    EndFile {
        entry_id: Option<i64>,
        reason: EndFileReason,
        file_error: Option<String>,
    },
    /// `data` is `None` when the property is unavailable (mpv sends no
    /// `data` field, e.g. `time-pos` while idle).
    PropertyChange {
        name: String,
        data: Option<Value>,
    },
    /// Any event Coda does not act on.
    Other {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MpvMessage {
    Reply {
        request_id: u64,
        /// `Ok(data)` when `error` is `"success"`, else `Err(error)`.
        result: Result<Option<Value>, String>,
    },
    Event(MpvEvent),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid JSON from mpv: {0}")]
    Json(#[from] serde_json::Error),
    #[error("message from mpv is neither a reply nor an event")]
    Unrecognized,
}

/// Every field mpv may send that Coda reads. Unknown fields are ignored.
#[derive(Deserialize)]
struct RawMessage {
    event: Option<String>,
    request_id: Option<u64>,
    error: Option<String>,
    data: Option<Value>,
    playlist_entry_id: Option<i64>,
    reason: Option<String>,
    file_error: Option<String>,
    name: Option<String>,
}

/// Parses one line received from mpv.
pub fn parse_line(line: &str) -> Result<MpvMessage, ProtocolError> {
    let raw: RawMessage = serde_json::from_str(line)?;
    if let Some(event) = raw.event {
        let event = match event.as_str() {
            "start-file" => MpvEvent::StartFile {
                entry_id: raw.playlist_entry_id,
            },
            "file-loaded" => MpvEvent::FileLoaded,
            "end-file" => MpvEvent::EndFile {
                entry_id: raw.playlist_entry_id,
                reason: match raw.reason.as_deref() {
                    Some("eof") => EndFileReason::Eof,
                    Some("stop") => EndFileReason::Stop,
                    Some("quit") => EndFileReason::Quit,
                    Some("error") => EndFileReason::Error,
                    Some("redirect") => EndFileReason::Redirect,
                    _ => EndFileReason::Unknown,
                },
                file_error: raw.file_error,
            },
            "property-change" => match raw.name {
                Some(name) => MpvEvent::PropertyChange {
                    name,
                    data: raw.data,
                },
                None => MpvEvent::Other { name: event },
            },
            _ => MpvEvent::Other { name: event },
        };
        return Ok(MpvMessage::Event(event));
    }
    match (raw.request_id, raw.error) {
        (Some(request_id), Some(error)) => Ok(MpvMessage::Reply {
            request_id,
            result: if error == "success" {
                Ok(raw.data)
            } else {
                Err(error)
            },
        }),
        _ => Err(ProtocolError::Unrecognized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_loadfile_with_hostile_path() {
        let path = "/music/-x \"quoted\" back\\slash\nnew line é 日本.flac";
        let line = encode_request(7, &MpvCommand::LoadFile { path: path.into() });
        assert_eq!(
            line,
            "{\"command\":[\"loadfile\",\"/music/-x \\\"quoted\\\" back\\\\slash\\nnew line é 日本.flac\",\"replace\"],\"request_id\":7}\n"
        );
        // Exactly one line, and it round-trips to the same path.
        assert_eq!(line.matches('\n').count(), 1);
        let v: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(v["command"][1], path);
    }

    #[test]
    fn encodes_leading_dash_as_plain_argument() {
        let line = encode_request(
            1,
            &MpvCommand::LoadFile {
                path: "--no-config".into(),
            },
        );
        assert_eq!(
            line,
            "{\"command\":[\"loadfile\",\"--no-config\",\"replace\"],\"request_id\":1}\n"
        );
    }

    #[test]
    fn encodes_other_commands() {
        let cases = [
            (
                MpvCommand::SetProperty {
                    name: "pause",
                    value: false.into(),
                },
                r#"{"command":["set_property","pause",false],"request_id":3}"#,
            ),
            (
                MpvCommand::SetProperty {
                    name: "volume",
                    value: 55.into(),
                },
                r#"{"command":["set_property","volume",55],"request_id":3}"#,
            ),
            (
                MpvCommand::GetProperty {
                    name: "playlist/0/id",
                },
                r#"{"command":["get_property","playlist/0/id"],"request_id":3}"#,
            ),
            (
                MpvCommand::ObserveProperty {
                    id: 2,
                    name: "time-pos",
                },
                r#"{"command":["observe_property",2,"time-pos"],"request_id":3}"#,
            ),
            (
                MpvCommand::Seek {
                    seconds: -5.0,
                    mode: SeekMode::Relative,
                },
                r#"{"command":["seek",-5.0,"relative"],"request_id":3}"#,
            ),
            (
                MpvCommand::Seek {
                    seconds: 0.0,
                    mode: SeekMode::Absolute,
                },
                r#"{"command":["seek",0.0,"absolute"],"request_id":3}"#,
            ),
            (MpvCommand::Stop, r#"{"command":["stop"],"request_id":3}"#),
            (MpvCommand::Quit, r#"{"command":["quit"],"request_id":3}"#),
        ];
        for (cmd, expected) in cases {
            assert_eq!(encode_request(3, &cmd), format!("{expected}\n"));
        }
    }

    #[test]
    fn describe_never_contains_path() {
        let cmd = MpvCommand::LoadFile {
            path: "/secret/path.flac".into(),
        };
        assert!(!cmd.describe().contains("secret"));
        assert!(!cmd.is_sensitive());
    }

    #[test]
    fn stream_loads_encode_the_raw_url_but_never_show_it() {
        let url = url::Url::parse("https://h.example/rest/stream?id=a&t=secret-token").unwrap();
        let cmd = MpvCommand::LoadStream {
            url: StreamUrl::new(url).unwrap(),
        };
        let line = encode_request(5, &cmd);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v["command"],
            serde_json::json!([
                "loadfile",
                "https://h.example/rest/stream?id=a&t=secret-token",
                "replace"
            ])
        );
        assert!(cmd.is_sensitive());
        assert_eq!(cmd.describe(), "loadfile");
        assert!(!format!("{cmd:?}").contains("secret"));
    }

    #[test]
    fn parses_replies() {
        // Shapes observed from mpv 0.41.
        assert_eq!(
            parse_line(r#"{"data": 1, "request_id": 4, "error": "success"}"#).unwrap(),
            MpvMessage::Reply {
                request_id: 4,
                result: Ok(Some(json!(1)))
            }
        );
        assert_eq!(
            parse_line(r#"{"request_id": 2, "error": "success"}"#).unwrap(),
            MpvMessage::Reply {
                request_id: 2,
                result: Ok(None)
            }
        );
        assert_eq!(
            parse_line(r#"{"data":{"playlist_entry_id":3},"request_id":9,"error":"success"}"#)
                .unwrap(),
            MpvMessage::Reply {
                request_id: 9,
                result: Ok(Some(json!({"playlist_entry_id": 3})))
            }
        );
        assert_eq!(
            parse_line(r#"{"request_id": 5, "error": "property not found"}"#).unwrap(),
            MpvMessage::Reply {
                request_id: 5,
                result: Err("property not found".into())
            }
        );
    }

    #[test]
    fn parses_file_events() {
        assert_eq!(
            parse_line(r#"{"event": "start-file", "playlist_entry_id": 2}"#).unwrap(),
            MpvMessage::Event(MpvEvent::StartFile { entry_id: Some(2) })
        );
        assert_eq!(
            parse_line(r#"{"event": "file-loaded"}"#).unwrap(),
            MpvMessage::Event(MpvEvent::FileLoaded)
        );
        assert_eq!(
            parse_line(r#"{"event": "end-file", "reason": "eof", "playlist_entry_id": 2}"#)
                .unwrap(),
            MpvMessage::Event(MpvEvent::EndFile {
                entry_id: Some(2),
                reason: EndFileReason::Eof,
                file_error: None
            })
        );
        assert_eq!(
            parse_line(
                r#"{"event": "end-file", "reason": "error", "playlist_entry_id": 3, "file_error": "loading failed"}"#
            )
            .unwrap(),
            MpvMessage::Event(MpvEvent::EndFile {
                entry_id: Some(3),
                reason: EndFileReason::Error,
                file_error: Some("loading failed".into())
            })
        );
        for (reason, expected) in [
            ("stop", EndFileReason::Stop),
            ("quit", EndFileReason::Quit),
            ("redirect", EndFileReason::Redirect),
            ("unknown", EndFileReason::Unknown),
            ("something-new", EndFileReason::Unknown),
        ] {
            let line =
                format!(r#"{{"event":"end-file","reason":"{reason}","playlist_entry_id":1}}"#);
            match parse_line(&line).unwrap() {
                MpvMessage::Event(MpvEvent::EndFile { reason: r, .. }) => assert_eq!(r, expected),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn parses_redirect_with_insert_fields() {
        // Shape observed from mpv 0.41 for a content-detected playlist.
        let line = r#"{"event": "end-file", "reason": "redirect", "playlist_entry_id": 1, "playlist_insert_id": 2, "playlist_insert_num_entries": 2}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            MpvMessage::Event(MpvEvent::EndFile {
                entry_id: Some(1),
                reason: EndFileReason::Redirect,
                file_error: None
            })
        );
    }

    #[test]
    fn parses_property_changes() {
        assert_eq!(
            parse_line(r#"{"event": "property-change", "id": 1, "name": "pause", "data": true}"#)
                .unwrap(),
            MpvMessage::Event(MpvEvent::PropertyChange {
                name: "pause".into(),
                data: Some(json!(true))
            })
        );
        assert_eq!(
            parse_line(r#"{"event": "property-change", "id": 5, "name": "time-pos"}"#).unwrap(),
            MpvMessage::Event(MpvEvent::PropertyChange {
                name: "time-pos".into(),
                data: None
            })
        );
    }

    #[test]
    fn unknown_events_and_fields_are_tolerated() {
        assert_eq!(
            parse_line(r#"{"event": "audio-reconfig"}"#).unwrap(),
            MpvMessage::Event(MpvEvent::Other {
                name: "audio-reconfig".into()
            })
        );
        assert_eq!(
            parse_line(r#"{"event":"start-file","playlist_entry_id":1,"future":[1,2]}"#).unwrap(),
            MpvMessage::Event(MpvEvent::StartFile { entry_id: Some(1) })
        );
    }

    #[test]
    fn malformed_lines_are_errors() {
        for line in [
            "",
            "not json",
            "{\"event\":",
            "[1,2,3]",
            "{}",
            r#"{"request_id": "x", "error": "success"}"#,
            r#"{"data": 1}"#,
        ] {
            assert!(parse_line(line).is_err(), "{line:?} should fail");
        }
    }
}
