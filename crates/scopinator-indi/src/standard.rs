//! The names of the standard INDI properties and their members, as the
//! INDI "Standard Properties" document gives them. A driver may define
//! any subset.

/// Properties every device has.
pub mod general {
    /// Switch: whether the driver is connected to its hardware.
    pub const CONNECTION: &str = "CONNECTION";
    pub const CONNECT: &str = "CONNECT";
    pub const DISCONNECT: &str = "DISCONNECT";
    /// Text: what the driver is.
    pub const DRIVER_INFO: &str = "DRIVER_INFO";
    pub const DRIVER_NAME: &str = "DRIVER_NAME";
    pub const DRIVER_EXEC: &str = "DRIVER_EXEC";
    pub const DRIVER_VERSION: &str = "DRIVER_VERSION";
    /// A decimal bit mask of [`interface`](crate::protocol::property::interface) flags.
    pub const DRIVER_INTERFACE: &str = "DRIVER_INTERFACE";
}

/// Mounts.
pub mod telescope {
    /// Numbers: where the mount points, in the equinox of date. Writing it
    /// does what `ON_COORD_SET` says.
    pub const EQUATORIAL_EOD_COORD: &str = "EQUATORIAL_EOD_COORD";
    /// Numbers: the same in J2000, on the few drivers that have it.
    pub const EQUATORIAL_COORD: &str = "EQUATORIAL_COORD";
    /// Hours.
    pub const RA: &str = "RA";
    /// Degrees.
    pub const DEC: &str = "DEC";
    /// Numbers: altitude and azimuth in degrees.
    pub const HORIZONTAL_COORD: &str = "HORIZONTAL_COORD";
    pub const ALT: &str = "ALT";
    pub const AZ: &str = "AZ";
    /// Switch: what writing coordinates does.
    pub const ON_COORD_SET: &str = "ON_COORD_SET";
    /// Go there and stop.
    pub const SLEW: &str = "SLEW";
    /// Go there and track.
    pub const TRACK: &str = "TRACK";
    /// Take them as where the mount is.
    pub const SYNC: &str = "SYNC";
    /// Switch: stop any motion.
    pub const TELESCOPE_ABORT_MOTION: &str = "TELESCOPE_ABORT_MOTION";
    pub const ABORT: &str = "ABORT";
    /// Switch: park and unpark.
    pub const TELESCOPE_PARK: &str = "TELESCOPE_PARK";
    pub const PARK: &str = "PARK";
    pub const UNPARK: &str = "UNPARK";
    /// Switch: tracking on or off.
    pub const TELESCOPE_TRACK_STATE: &str = "TELESCOPE_TRACK_STATE";
    pub const TRACK_ON: &str = "TRACK_ON";
    pub const TRACK_OFF: &str = "TRACK_OFF";
    /// Switch: the tracking rate.
    pub const TELESCOPE_TRACK_MODE: &str = "TELESCOPE_TRACK_MODE";
    pub const TRACK_SIDEREAL: &str = "TRACK_SIDEREAL";
    pub const TRACK_SOLAR: &str = "TRACK_SOLAR";
    pub const TRACK_LUNAR: &str = "TRACK_LUNAR";
    pub const TRACK_CUSTOM: &str = "TRACK_CUSTOM";
    /// Switch: move north or south for as long as a member is on.
    pub const TELESCOPE_MOTION_NS: &str = "TELESCOPE_MOTION_NS";
    pub const MOTION_NORTH: &str = "MOTION_NORTH";
    pub const MOTION_SOUTH: &str = "MOTION_SOUTH";
    /// Switch: move west or east for as long as a member is on.
    pub const TELESCOPE_MOTION_WE: &str = "TELESCOPE_MOTION_WE";
    pub const MOTION_WEST: &str = "MOTION_WEST";
    pub const MOTION_EAST: &str = "MOTION_EAST";
    /// Switch: the speed of those moves, its members from slowest to
    /// fastest.
    pub const TELESCOPE_SLEW_RATE: &str = "TELESCOPE_SLEW_RATE";
    /// Numbers: a guide pulse north or south, in milliseconds.
    pub const TELESCOPE_TIMED_GUIDE_NS: &str = "TELESCOPE_TIMED_GUIDE_NS";
    pub const TIMED_GUIDE_N: &str = "TIMED_GUIDE_N";
    pub const TIMED_GUIDE_S: &str = "TIMED_GUIDE_S";
    /// Numbers: a guide pulse west or east, in milliseconds.
    pub const TELESCOPE_TIMED_GUIDE_WE: &str = "TELESCOPE_TIMED_GUIDE_WE";
    pub const TIMED_GUIDE_W: &str = "TIMED_GUIDE_W";
    pub const TIMED_GUIDE_E: &str = "TIMED_GUIDE_E";
    /// Switch: which side of the pier the telescope is on.
    pub const TELESCOPE_PIER_SIDE: &str = "TELESCOPE_PIER_SIDE";
    pub const PIER_EAST: &str = "PIER_EAST";
    pub const PIER_WEST: &str = "PIER_WEST";
    /// Numbers: the site. Longitude runs 0 to 360, east positive.
    pub const GEOGRAPHIC_COORD: &str = "GEOGRAPHIC_COORD";
    pub const LAT: &str = "LAT";
    pub const LONG: &str = "LONG";
    pub const ELEV: &str = "ELEV";
    /// Text: the time, as ISO 8601 UTC and an offset in hours.
    pub const TIME_UTC: &str = "TIME_UTC";
    pub const UTC: &str = "UTC";
    pub const OFFSET: &str = "OFFSET";
}

/// Cameras.
pub mod ccd {
    /// Number: start an exposure of this many seconds; counts down.
    pub const CCD_EXPOSURE: &str = "CCD_EXPOSURE";
    pub const CCD_EXPOSURE_VALUE: &str = "CCD_EXPOSURE_VALUE";
    /// Switch: abort the exposure.
    pub const CCD_ABORT_EXPOSURE: &str = "CCD_ABORT_EXPOSURE";
    pub const ABORT: &str = "ABORT";
    /// Numbers: the sensor.
    pub const CCD_INFO: &str = "CCD_INFO";
    pub const CCD_MAX_X: &str = "CCD_MAX_X";
    pub const CCD_MAX_Y: &str = "CCD_MAX_Y";
    pub const CCD_PIXEL_SIZE: &str = "CCD_PIXEL_SIZE";
    pub const CCD_PIXEL_SIZE_X: &str = "CCD_PIXEL_SIZE_X";
    pub const CCD_PIXEL_SIZE_Y: &str = "CCD_PIXEL_SIZE_Y";
    pub const CCD_BITSPERPIXEL: &str = "CCD_BITSPERPIXEL";
    /// Numbers: the region read, in unbinned pixels.
    pub const CCD_FRAME: &str = "CCD_FRAME";
    pub const X: &str = "X";
    pub const Y: &str = "Y";
    pub const WIDTH: &str = "WIDTH";
    pub const HEIGHT: &str = "HEIGHT";
    /// Numbers: binning.
    pub const CCD_BINNING: &str = "CCD_BINNING";
    pub const HOR_BIN: &str = "HOR_BIN";
    pub const VER_BIN: &str = "VER_BIN";
    /// Switch: the kind of frame.
    pub const CCD_FRAME_TYPE: &str = "CCD_FRAME_TYPE";
    pub const FRAME_LIGHT: &str = "FRAME_LIGHT";
    pub const FRAME_BIAS: &str = "FRAME_BIAS";
    pub const FRAME_DARK: &str = "FRAME_DARK";
    pub const FRAME_FLAT: &str = "FRAME_FLAT";
    /// Number: the sensor temperature in degrees Celsius; writing it sets
    /// the target.
    pub const CCD_TEMPERATURE: &str = "CCD_TEMPERATURE";
    pub const CCD_TEMPERATURE_VALUE: &str = "CCD_TEMPERATURE_VALUE";
    /// Switch: the cooler.
    pub const CCD_COOLER: &str = "CCD_COOLER";
    pub const COOLER_ON: &str = "COOLER_ON";
    pub const COOLER_OFF: &str = "COOLER_OFF";
    /// Number: cooler power in percent.
    pub const CCD_COOLER_POWER: &str = "CCD_COOLER_POWER";
    pub const CCD_COOLER_VALUE: &str = "CCD_COOLER_VALUE";
    /// Number: gain, on drivers that have it as its own property.
    pub const CCD_GAIN: &str = "CCD_GAIN";
    pub const GAIN: &str = "GAIN";
    /// Number: offset, likewise.
    pub const CCD_OFFSET: &str = "CCD_OFFSET";
    pub const OFFSET: &str = "OFFSET";
    /// Numbers: gain, offset and more on drivers that group them.
    pub const CCD_CONTROLS: &str = "CCD_CONTROLS";
    /// Text: the colour filter array.
    pub const CCD_CFA: &str = "CCD_CFA";
    pub const CFA_OFFSET_X: &str = "CFA_OFFSET_X";
    pub const CFA_OFFSET_Y: &str = "CFA_OFFSET_Y";
    pub const CFA_TYPE: &str = "CFA_TYPE";
    /// BLOB: the image, both the property and its one member.
    pub const CCD1: &str = "CCD1";
}

/// Focusers.
pub mod focuser {
    /// Number: the position in steps; writing it moves there.
    pub const ABS_FOCUS_POSITION: &str = "ABS_FOCUS_POSITION";
    pub const FOCUS_ABSOLUTE_POSITION: &str = "FOCUS_ABSOLUTE_POSITION";
    /// Number: move this many steps in the `FOCUS_MOTION` direction.
    pub const REL_FOCUS_POSITION: &str = "REL_FOCUS_POSITION";
    pub const FOCUS_RELATIVE_POSITION: &str = "FOCUS_RELATIVE_POSITION";
    /// Switch: the direction of relative moves.
    pub const FOCUS_MOTION: &str = "FOCUS_MOTION";
    pub const FOCUS_INWARD: &str = "FOCUS_INWARD";
    pub const FOCUS_OUTWARD: &str = "FOCUS_OUTWARD";
    /// Switch: stop.
    pub const FOCUS_ABORT_MOTION: &str = "FOCUS_ABORT_MOTION";
    pub const ABORT: &str = "ABORT";
    /// Number: the far end of the travel.
    pub const FOCUS_MAX: &str = "FOCUS_MAX";
    pub const FOCUS_MAX_VALUE: &str = "FOCUS_MAX_VALUE";
    /// Number: degrees Celsius.
    pub const FOCUS_TEMPERATURE: &str = "FOCUS_TEMPERATURE";
    pub const TEMPERATURE: &str = "TEMPERATURE";
}

/// Filter wheels.
pub mod filter_wheel {
    /// Number: the slot in use, **counted from one**; writing it turns the
    /// wheel.
    pub const FILTER_SLOT: &str = "FILTER_SLOT";
    pub const FILTER_SLOT_VALUE: &str = "FILTER_SLOT_VALUE";
    /// Text: one member for each slot, in slot order.
    pub const FILTER_NAME: &str = "FILTER_NAME";
}
