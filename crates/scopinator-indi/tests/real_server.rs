//! What a real `indiserver` sent, read by the parser.
//!
//! The fixtures are captures from `indiserver` 1.9.9 running the telescope,
//! CCD, focuser and filter wheel simulators: the reply to `getProperties`,
//! and the same with each device then asked to connect. They are as the
//! server sent them except that the home directory in three paths has been
//! replaced with `/home/observer`.

use std::collections::BTreeMap;

use scopinator_indi::protocol::message::{IndiMessage, MemberValue, Permission, VectorKind};
use scopinator_indi::protocol::parse::StreamParser;
use scopinator_indi::standard::{ccd, filter_wheel, focuser, general, telescope};

const DEFINED: &[u8] = include_bytes!("fixtures/indiserver-1.9.9-simulators-defined.xml");
const CONNECTED: &[u8] = include_bytes!("fixtures/indiserver-1.9.9-simulators-connected.xml");

/// Every message in `capture`, fed in pieces of `chunk` bytes.
fn read(capture: &[u8], chunk: usize) -> Vec<IndiMessage> {
    let mut parser = StreamParser::new();
    capture
        .chunks(chunk)
        .flat_map(|piece| parser.feed(piece))
        .map(|message| message.expect("a real server's message"))
        .collect()
}

#[test]
fn every_message_of_a_real_server_is_read() {
    let defined = read(DEFINED, 4096);
    assert_eq!(defined.len(), 62);
    assert!(defined.iter().all(|m| matches!(m, IndiMessage::Define(_))));

    let connected = read(CONNECTED, 4096);
    let count = |wanted: fn(&IndiMessage) -> bool| connected.iter().filter(|m| wanted(m)).count();
    assert_eq!(count(|m| matches!(m, IndiMessage::Define(_))), 159);
    assert_eq!(count(|m| matches!(m, IndiMessage::Update(_))), 32);
    assert_eq!(count(|m| matches!(m, IndiMessage::Message { .. })), 4);
    assert_eq!(count(|m| matches!(m, IndiMessage::Unknown { .. })), 0);
}

#[test]
fn how_the_bytes_are_split_does_not_matter() {
    let whole = read(CONNECTED, CONNECTED.len());
    for chunk in [1, 7, 100, 1460] {
        assert_eq!(read(CONNECTED, chunk), whole, "in pieces of {chunk}");
    }
}

#[test]
fn the_standard_properties_are_there_under_the_names_this_library_uses() {
    let mut properties = BTreeMap::new();
    for message in read(CONNECTED, 4096) {
        if let IndiMessage::Define(property) = message {
            properties.insert((property.device.clone(), property.name.clone()), property);
        }
    }
    let members = |device: &str, name: &str| -> Vec<String> {
        properties
            .get(&(device.to_string(), name.to_string()))
            .unwrap_or_else(|| panic!("{device} has no {name}"))
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect()
    };
    let scope = "Telescope Simulator";
    assert_eq!(
        members(scope, general::CONNECTION),
        [general::CONNECT, general::DISCONNECT]
    );
    assert_eq!(
        members(scope, general::DRIVER_INFO),
        [
            general::DRIVER_NAME,
            general::DRIVER_EXEC,
            general::DRIVER_VERSION,
            general::DRIVER_INTERFACE
        ]
    );
    assert_eq!(
        members(scope, telescope::EQUATORIAL_EOD_COORD),
        [telescope::RA, telescope::DEC]
    );
    assert_eq!(
        members(scope, telescope::ON_COORD_SET),
        [telescope::TRACK, telescope::SLEW, telescope::SYNC]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_PARK),
        [telescope::PARK, telescope::UNPARK]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_ABORT_MOTION),
        [telescope::ABORT]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_TRACK_STATE),
        [telescope::TRACK_ON, telescope::TRACK_OFF]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_TRACK_MODE),
        [
            telescope::TRACK_SIDEREAL,
            telescope::TRACK_SOLAR,
            telescope::TRACK_LUNAR,
            telescope::TRACK_CUSTOM
        ]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_MOTION_NS),
        [telescope::MOTION_NORTH, telescope::MOTION_SOUTH]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_MOTION_WE),
        [telescope::MOTION_WEST, telescope::MOTION_EAST]
    );
    // Slowest first.
    assert_eq!(
        members(scope, telescope::TELESCOPE_SLEW_RATE),
        ["1x", "2x", "3x", "4x"]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_TIMED_GUIDE_NS),
        [telescope::TIMED_GUIDE_N, telescope::TIMED_GUIDE_S]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_TIMED_GUIDE_WE),
        [telescope::TIMED_GUIDE_W, telescope::TIMED_GUIDE_E]
    );
    assert_eq!(
        members(scope, telescope::TELESCOPE_PIER_SIDE),
        [telescope::PIER_WEST, telescope::PIER_EAST]
    );
    assert_eq!(
        members(scope, telescope::GEOGRAPHIC_COORD),
        [telescope::LAT, telescope::LONG, telescope::ELEV]
    );
    assert_eq!(
        members(scope, telescope::TIME_UTC),
        [telescope::UTC, telescope::OFFSET]
    );

    let camera = "CCD Simulator";
    assert_eq!(
        members(camera, ccd::CCD_EXPOSURE),
        [ccd::CCD_EXPOSURE_VALUE]
    );
    assert_eq!(members(camera, ccd::CCD_ABORT_EXPOSURE), [ccd::ABORT]);
    assert_eq!(
        members(camera, ccd::CCD_INFO),
        [
            ccd::CCD_MAX_X,
            ccd::CCD_MAX_Y,
            ccd::CCD_PIXEL_SIZE,
            ccd::CCD_PIXEL_SIZE_X,
            ccd::CCD_PIXEL_SIZE_Y,
            ccd::CCD_BITSPERPIXEL
        ]
    );
    assert_eq!(
        members(camera, ccd::CCD_FRAME),
        [ccd::X, ccd::Y, ccd::WIDTH, ccd::HEIGHT]
    );
    assert_eq!(
        members(camera, ccd::CCD_BINNING),
        [ccd::HOR_BIN, ccd::VER_BIN]
    );
    assert_eq!(
        members(camera, ccd::CCD_FRAME_TYPE),
        [
            ccd::FRAME_LIGHT,
            ccd::FRAME_BIAS,
            ccd::FRAME_DARK,
            ccd::FRAME_FLAT
        ]
    );
    assert_eq!(
        members(camera, ccd::CCD_TEMPERATURE),
        [ccd::CCD_TEMPERATURE_VALUE]
    );
    assert_eq!(
        members(camera, ccd::CCD_COOLER),
        [ccd::COOLER_ON, ccd::COOLER_OFF]
    );
    assert_eq!(members(camera, ccd::CCD_GAIN), [ccd::GAIN]);
    assert_eq!(members(camera, ccd::CCD_OFFSET), [ccd::OFFSET]);
    assert_eq!(members(camera, ccd::CCD1), [ccd::CCD1]);
    let image = &properties[&(camera.to_string(), ccd::CCD1.to_string())];
    assert_eq!(image.kind, VectorKind::Blob);
    assert_eq!(image.members[0].value, MemberValue::NoBlob);
    // The simulator's cooler can be switched and not read.
    assert_eq!(
        properties[&(camera.to_string(), ccd::CCD_COOLER.to_string())].permission,
        Permission::WriteOnly
    );

    let focus = "Focuser Simulator";
    assert_eq!(
        members(focus, focuser::ABS_FOCUS_POSITION),
        [focuser::FOCUS_ABSOLUTE_POSITION]
    );
    assert_eq!(
        members(focus, focuser::REL_FOCUS_POSITION),
        [focuser::FOCUS_RELATIVE_POSITION]
    );
    assert_eq!(
        members(focus, focuser::FOCUS_MOTION),
        [focuser::FOCUS_INWARD, focuser::FOCUS_OUTWARD]
    );
    assert_eq!(
        members(focus, focuser::FOCUS_MAX),
        [focuser::FOCUS_MAX_VALUE]
    );
    assert_eq!(
        members(focus, focuser::FOCUS_TEMPERATURE),
        [focuser::TEMPERATURE]
    );

    let wheel = "Filter Simulator";
    assert_eq!(
        members(wheel, filter_wheel::FILTER_SLOT),
        [filter_wheel::FILTER_SLOT_VALUE]
    );
    assert_eq!(members(wheel, filter_wheel::FILTER_NAME).len(), 8);
    let slot = &properties[&(wheel.to_string(), filter_wheel::FILTER_SLOT.to_string())];
    let limits = slot.members[0].limits.as_ref().unwrap();
    // Slots count from one.
    assert_eq!((limits.min, limits.max), (1.0, 8.0));
    // A camera with a wheel built in has the wheel's properties too.
    assert_eq!(
        members(camera, filter_wheel::FILTER_SLOT),
        [filter_wheel::FILTER_SLOT_VALUE]
    );
}
