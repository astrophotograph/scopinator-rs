# The Alpaca backend

`scopinator` drives ASCOM Alpaca devices through the same traits as a Seestar: a server's
telescope, cameras, focuser and filter wheel become a `Backend` with a `Mount`, `Camera`s, a
`Focuser` and a `FilterWheel` (feature `alpaca`).

```rust
let manager = scopinator::DeviceManager::new();
let id = manager.add_alpaca("192.168.1.50".parse()?, 11111).await?;
let observatory = manager.get(&id).expect("just added");
if let Some(mount) = observatory.mount() {
    mount.slew_to_coordinates(&target).await?;          // returns when accepted
    scopinator::wait_for_slew(mount.as_ref(), Default::default()).await?;
}
```

The command-line tool takes an Alpaca server as `--host alpaca://HOST:PORT` (or `--backend
alpaca --port PORT` with a bare address):

```
scopinator discover
scopinator status --host alpaca://192.168.1.50:11111
scopinator unpark --host alpaca://192.168.1.50:11111
scopinator goto   --host alpaca://192.168.1.50:11111 --ra 5.588 --dec -5.39
scopinator park   --host alpaca://192.168.1.50:11111
```

`goto` takes J2000 coordinates (`--jnow` for the equinox of date), turns tracking on if it is
off, waits for the mount to arrive and prints where it ended up. `raw` and the clock options
of `goto` are for a Seestar only.

Alpaca has no events, so the backend polls its devices (once a second by default,
`AlpacaConfig::poll_interval`) and puts the same events on the bus that a Seestar reports by
itself.

## Testing against the ASCOM simulators

The backend's ordinary tests run against a fake server that lives in this repository
(`crates/scopinator/tests/alpaca_common`). It was written by the same hands as the client, so
it cannot catch a misreading of the Alpaca API. For that there is a second suite,
`crates/scopinator/tests/alpaca_omnisim.rs`, which runs against the
[ASCOM Alpaca Omni Simulators](https://github.com/ASCOMInitiative/ASCOM.Alpaca.Simulators), the
reference simulator the ASCOM Initiative publishes.

```
just alpaca-sim-test        # or: tools/alpaca-sim.sh
```

This downloads the pinned release into `~/.cache/scopinator/omnisim` (34 MB, once; the
x86-64 Linux archive's SHA-256 is checked), starts the simulator bound to `127.0.0.1:32323`
with an empty home directory so that it comes up in its default state and writes nothing to
your own `~/.ASCOM`, runs the suite one test at a time, and stops the simulator. The build is
self-contained: no .NET install is needed.

`tools/alpaca-sim.sh start` leaves the simulator running for manual work
(`tools/alpaca-sim.sh test` runs the suite against it, `stop` ends it). `ALPACA_SIM_PORT` and
`ALPACA_SIM_CACHE` change the port and the cache directory. To use a simulator started some
other way, set `ALPACA_SIM_URL=http://<ipv4>:<port>` and run

```
cargo test -p scopinator --features alpaca --test alpaca_omnisim -- --ignored --test-threads=1
```

The simulator also answers Alpaca discovery broadcasts while it runs, so other Alpaca clients on
the network will see it.

The suite runs nightly in CI (`.github/workflows/extended.yml`, job `alpaca-sim`).

### What it covers

Through the neutral traits only, against OmniSim **v0.4.0** on Linux x86-64:

- the server's devices are found and connected; capabilities are read;
- mount: unpark, tracking, a slew to a J2000 position (converted to the simulator's
  equinox-of-date system) that returns on acceptance and is waited for, arrival within one
  arcminute, sync, manual axis motion at several speeds, site and clock, park and unpark, and
  the refusal of a slew while parked;
- camera: a binned subframe and a full frame through the ImageBytes download, with the
  image's dimensions and length checked;
- focuser and filter wheel: a move each, waited for, and out-of-range requests refused;
- the poller: one `SlewStarted` and one `SlewCompleted` per slew, `Parked`, `ImageReady`,
  `FocuserMoveCompleted`, `FilterChanged`.

### What it found

- **Axis rates come in separate ranges.** The simulated mount offers 0 to 6.67 and 10 to 20
  degrees per second. `Mount::move_axis` took a fraction of the fastest rate, and 0.4 of 20 is
  8, which the mount refuses. A speed that lands between two ranges now goes to the nearest
  rate the mount accepts.
- **A moving axis counts as slewing.** After `move_axis`, Alpaca's `Slewing` stays true until
  the axis is stopped, so `wait_for_slew` on a mount that was left moving never returns.

### What it does not cover

- Image orientation. The suite checks dimensions, not content: the simulator's default image
  has no feature this suite looks for. That the first array dimension is the width follows
  the ASCOM camera interface and the reference library's code.
- Colour and Bayer sensors (the simulated sensor is monochrome), cooling set points, pulse
  guiding, alt-az slews, and a mount that works in J2000.
- Any real driver. Drivers differ from the simulator in what they leave unimplemented.
