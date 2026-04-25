//! INDI XML message types.
//!
//! The INDI protocol uses XML messages over TCP. This module defines
//! the message types for serialization and deserialization.

use crate::protocol::property::SwitchState;

/// An INDI XML command to send to the server.
#[derive(Debug, Clone)]
pub enum IndiCommand {
    /// Get properties for a device (or all devices if device is None).
    GetProperties {
        device: Option<String>,
        name: Option<String>,
    },
    /// Set a new number property value.
    NewNumber {
        device: String,
        name: String,
        values: Vec<(String, f64)>,
    },
    /// Set a new switch property value.
    NewSwitch {
        device: String,
        name: String,
        values: Vec<(String, SwitchState)>,
    },
    /// Set a new text property value.
    NewText {
        device: String,
        name: String,
        values: Vec<(String, String)>,
    },
    /// Enable BLOB mode for a device.
    EnableBlob {
        device: String,
        name: Option<String>,
        mode: BlobMode,
    },
}

/// BLOB transfer modes.
#[derive(Debug, Clone, Copy)]
pub enum BlobMode {
    Never,
    Also,
    Only,
}

impl BlobMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::Also => "Also",
            Self::Only => "Only",
        }
    }
}

/// Serialize an INDI command to XML.
pub fn serialize_command(cmd: &IndiCommand) -> String {
    match cmd {
        IndiCommand::GetProperties { device, name } => {
            let mut xml = String::from("<getProperties version=\"1.7\"");
            if let Some(d) = device {
                xml.push_str(&format!(" device=\"{d}\""));
            }
            if let Some(n) = name {
                xml.push_str(&format!(" name=\"{n}\""));
            }
            xml.push_str("/>\n");
            xml
        }

        IndiCommand::NewNumber {
            device,
            name,
            values,
        } => {
            let mut xml = format!("<newNumberVector device=\"{device}\" name=\"{name}\">\n");
            for (vname, value) in values {
                xml.push_str(&format!(
                    "  <oneNumber name=\"{vname}\">{value}</oneNumber>\n"
                ));
            }
            xml.push_str("</newNumberVector>\n");
            xml
        }

        IndiCommand::NewSwitch {
            device,
            name,
            values,
        } => {
            let mut xml = format!("<newSwitchVector device=\"{device}\" name=\"{name}\">\n");
            for (vname, state) in values {
                let s = match state {
                    SwitchState::On => "On",
                    SwitchState::Off => "Off",
                };
                xml.push_str(&format!("  <oneSwitch name=\"{vname}\">{s}</oneSwitch>\n"));
            }
            xml.push_str("</newSwitchVector>\n");
            xml
        }

        IndiCommand::NewText {
            device,
            name,
            values,
        } => {
            let mut xml = format!("<newTextVector device=\"{device}\" name=\"{name}\">\n");
            for (vname, value) in values {
                xml.push_str(&format!("  <oneText name=\"{vname}\">{value}</oneText>\n"));
            }
            xml.push_str("</newTextVector>\n");
            xml
        }

        IndiCommand::EnableBlob { device, name, mode } => {
            let mut xml = format!("<enableBLOB device=\"{device}\"");
            if let Some(n) = name {
                xml.push_str(&format!(" name=\"{n}\""));
            }
            xml.push_str(&format!(">{}</enableBLOB>\n", mode.as_str()));
            xml
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_get_properties() {
        let cmd = IndiCommand::GetProperties {
            device: None,
            name: None,
        };
        let xml = serialize_command(&cmd);
        assert_eq!(xml, "<getProperties version=\"1.7\"/>\n");
    }

    #[test]
    fn serialize_get_properties_with_device() {
        let cmd = IndiCommand::GetProperties {
            device: Some("EQMod Mount".into()),
            name: None,
        };
        let xml = serialize_command(&cmd);
        assert!(xml.contains("device=\"EQMod Mount\""));
    }

    #[test]
    fn serialize_new_number() {
        let cmd = IndiCommand::NewNumber {
            device: "EQMod Mount".into(),
            name: "EQUATORIAL_EOD_COORD".into(),
            values: vec![("RA".into(), 12.5), ("DEC".into(), 45.0)],
        };
        let xml = serialize_command(&cmd);
        assert!(xml.contains("<newNumberVector"));
        assert!(xml.contains("name=\"RA\">12.5</oneNumber>"));
        assert!(xml.contains("name=\"DEC\">45</oneNumber>"));
    }

    #[test]
    fn serialize_new_switch() {
        let cmd = IndiCommand::NewSwitch {
            device: "EQMod Mount".into(),
            name: "CONNECTION".into(),
            values: vec![
                ("CONNECT".into(), SwitchState::On),
                ("DISCONNECT".into(), SwitchState::Off),
            ],
        };
        let xml = serialize_command(&cmd);
        assert!(xml.contains("name=\"CONNECT\">On</oneSwitch>"));
        assert!(xml.contains("name=\"DISCONNECT\">Off</oneSwitch>"));
    }

    #[test]
    fn serialize_enable_blob() {
        let cmd = IndiCommand::EnableBlob {
            device: "ZWO CCD".into(),
            name: Some("CCD1".into()),
            mode: BlobMode::Also,
        };
        let xml = serialize_command(&cmd);
        assert!(xml.contains("device=\"ZWO CCD\""));
        assert!(xml.contains("name=\"CCD1\""));
        assert!(xml.contains(">Also</enableBLOB>"));
    }
}
