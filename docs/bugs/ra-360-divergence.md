# RA 360° / 24h and Alt/Az validation: Rust and Python differ

Filed from Forge task "Add differential coordinate tests mirroring core types oracle".
Status: **open, not adjudicated.** This report describes both behaviours and gives no verdict.

## Python (oracle)

`pyscopinator/src/scopinator/v2/core/types.py`, `Coordinates` (exercised by
`tests/v2/test_core_types.py::TestCoordinates::test_ra_validation`):

- `ra` is a `Field(ge=0, lt=360)` in degrees. `ra=360.0` raises `ValueError`.
- `AltAzCoordinates` validates altitude in `-90..=90` and azimuth in `0..<360`
  (`TestAltAzCoordinates::test_altitude_validation`, `::test_azimuth_validation`).
- `Coordinates.from_hours(ra_hours=24.0, ...)` yields ra=360.0, so it also raises.

## Rust (`crates/scopinator-types/src/coordinates.rs`)

- `RaDegrees::new(360.0)` returns `Ok`, normalized to `0.0`. Values beyond 360 (+ epsilon) or below 0 are `Err`.
- `RaDegrees::from_hours(24.0)` and `Coordinates::from_hours(24.0, _)` return `Ok` with RA `0.0`.
  Hours above 24 or below 0 are `Err(InvalidRaHours)`.
- `AltAzCoordinates` has public `altitude`/`azimuth` fields and no constructor; no range is
  enforced, so Python-invalid values (alt 91, az 360, az -1) are representable.

## Pinned by tests

`py_divergence_ra_360_normalizes_to_zero` and `py_divergence_altaz_has_no_validation` in
`coordinates.rs` assert today's Rust behaviour. They will fail, on purpose, if either side changes.

## Who relies on what

Callers were not exhaustively audited; to be filled in during adjudication
(`grep -rn "RaDegrees::\|from_hours\|AltAzCoordinates" crates/`). Considerations on each side:

- Normalizing 360 -> 0 tolerates a mount or plate-solver reporting RA 24h/360° at the wrap point,
  which is the same sky position as 0.
- Rejecting 360 matches the Python half-open range, so a value accepted by one port is accepted by the other.
- Un-validated Alt/Az means Rust cannot reject bad device-reported angles at construction.
