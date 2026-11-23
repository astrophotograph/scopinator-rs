# The device contract

What every backend promises about the vendor-neutral traits in `scopinator::device`
(`Device`, `Mount`, `Camera`, `Focuser`, `FilterWheel`), whatever telescope is behind them.

## Operations that take time

A slew, a park, an exposure and a focuser or filter wheel move all outlast the request that
starts them. Protocols disagree about what a reply means: ASCOM Alpaca replies at once and has
the client poll `Slewing`; a Seestar replies at once and reports progress in events. The traits
hide the difference with one rule.

1. **Starting returns on acceptance.** `Mount::slew_to_coordinates`, `Mount::park`,
   `Camera::start_exposure`, `Focuser::move_to`, `Focuser::move_relative` and
   `FilterWheel::set_position` return `Ok(())` when the device has accepted the request. They
   never wait for the operation to finish. An error means the request was refused and nothing
   was started.

2. **A state query says whether it is still running.**

   | Operation | Query | Finished when |
   |---|---|---|
   | slew | `Mount::is_slewing` | `false` |
   | park | `Mount::is_parked` | `true` |
   | exposure | `Camera::image_ready` | `true` |
   | focuser move | `Focuser::is_moving` | `false` |
   | filter change | `FilterWheel::is_moving` | `false` |

   `is_slewing` is true from acceptance until the mount is ready to use. On a telescope that
   centres its target by plate solving, that includes the centring. A backend whose device
   takes a moment to start moving must still answer `true` in that gap.

3. **Failure after acceptance comes back from the query.** If the operation fails once under
   way, the next call to its state query returns the error, and calls after that return the
   idle answer. The backend also emits an `Error` event.

4. **A device that cannot tell says so.** A query returns `ScopinatorError::NotSupported` when
   the device has no way to report the state. It does not guess.

5. **Backends do not wait; callers do.** Waiting is built once on top of the queries and works
   for every backend. A backend that also has events emits them (`SlewStarted`,
   `SlewCompleted`, ...); a backend whose protocol has none polls and emits the same events.

## Identity

Every device has an id (`Device::id`) that is unique among the devices a program has open. The
devices of one instrument share its id as a prefix: `seestar:192.168.1.100:mount`,
`seestar:192.168.1.100:camera`. Events carry the instrument's id.
