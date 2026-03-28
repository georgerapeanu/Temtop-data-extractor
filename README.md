# temtop-sensor

Disclaimer
------------
This is 100% vibecoded.

Minimal Rust CLI for Temtop BLE sensors, starting with the `C1+`.

Current capabilities:

- scan for nearby Temtop sensors and infer MAC addresses and GUIDs from advertising names
- inspect services and characteristics
- read device parameters
- read one current measurement snapshot
- stream live measurements
- fetch device-local history records

This project is deliberately structured to be extended with support for additional Temtop models that expose different readings or packet layouts.

## Why this repo exists

The immediate goal is reliable local BLE access to Temtop sensors.
The CLI already supports the discovery patterns. 

- infer GUIDs and sensor model from advertising names when scanning
- resolve a device by MAC address, GUID, or both
- keep model-specific parsing isolated from shared BLE transport

For `C1+`, `--guid` is usually optional because it can be inferred from
advertising names like `C1+_90158797465673526885`. You only need to pass
`--guid` explicitly when the advertisement name is missing or unavailable on the
local platform.

## Reverse-engineering provenance

The current `C1+` constants are not arbitrary. They come from the decompiled APK in:

- `com.elitech.environment.en.util.BleSendParse`
- `com.elitech.environment.en.device.DeviceDetailActivityC1Plus`
- `defpackage.al0`
- `defpackage.si1`

Those files are the basis for:

- request/response frame headers and checksum rules
- decimal two-digit GUID encoding
- command IDs like `0x84`, `0x87`, `0x88`, and `0xe1`
- service / notify / write UUIDs
- `C1+_<guid>` advertising-name matching
- current and history field offsets

The Rust source includes short comments pointing back to those classes so the packet constants can be revalidated later.

## Sensor support model

Sensor-specific behavior lives behind a profile layer in:

- `src/sensor/mod.rs`
- `src/sensor/c1_plus.rs`

To add a new Temtop sensor later:

1. create a new `src/sensor/<model>.rs`
2. implement the `SensorProfile` trait
3. register it in `src/sensor/mod.rs`

That keeps BLE transport logic shared while allowing each sensor to define:

- advertised-name matching
- GUID inference
- service and characteristic UUIDs
- params/current/history parsers

The intent is that adding a new model should usually mean adding one new profile file, not rewriting the CLI.

## Efficiency choices

The release build is tuned for execution efficiency first:

- `lto = "fat"`
- `codegen-units = 1`
- `opt-level = 3`
- `panic = "abort"`
- `strip = true`

Runtime behavior is conservative as well:

- scans stop as soon as the target device is resolved
- `live` mode uses notifications and only polls when asked
- parsing is model-specific and allocation-light
- history output creates its parent directory automatically

The project still favors clean structure over chasing every last byte.

## Build

### Cargo

```bash
cargo build --release
```

Binary:

```bash
./target/release/temtop-sensor
```

### Nix

This repository includes a flake that provides:

- `packages.default` for the `temtop-sensor` binary
- `apps.default` for `nix run`
- `devShells.default` with the Rust and BLE build dependencies used in CI

Typical usage:

```bash
nix build
./result/bin/temtop-sensor
```

```bash
nix run
```

```bash
nix develop
```

## Commands

The top-level CLI currently exposes:

- `scan`
- `c1plus`

The `c1plus` subcommand contains the sensor-specific operations for the currently supported model.

### Scan

Scan for nearby Temtop devices and infer GUIDs from names like `C1+_90158797465673526885`.

```bash
cargo run --release -- scan --seconds 10
```

Show every nearby BLE device instead of only Temtop-looking advertisements:

```bash
cargo run --release -- scan --seconds 10 --show-all
```

JSON output:

```bash
cargo run --release -- scan --seconds 10 --format json
```

### Inspect

Resolve a sensor by address or GUID, connect, and print services and characteristics.

```bash
cargo run --release -- c1plus inspect --address A4:C1:38:C2:CC:F2
```

If a device is known by MAC address but its advertising name is temporarily absent, the CLI still uses the selected sensor profile to connect and inspect it.

### Params

Read device configuration/telemetry currently exposed via the BLE params response.

```bash
cargo run --release -- c1plus params \
  --address A4:C1:38:C2:CC:F2 \
  --format json
```

For `C1+`, this includes:

- battery
- temperature unit
- log interval
- AQI standard
- alarm settings
- time mode
- firmware/version code
- work mode

### Current

Read a single current sample.

```bash
cargo run --release -- c1plus current \
  --address A4:C1:38:C2:CC:F2 \
  --format json
```

For `C1+`, this currently parses:

- PM2.5
- temperature
- humidity
- AQI
- CO2
- TVOC
- battery
- temperature unit

### Live

Subscribe to live notifications and optionally poll current data periodically.

```bash
cargo run --release -- c1plus live \
  --address A4:C1:38:C2:CC:F2 \
  --poll-secs 10 \
  --format jsonl
```

Notes:

- `--poll-secs 0` disables active polling and only listens for push notifications
- output is one JSON object per line in `--format jsonl` mode, which is intentionally easy to pipe later

### History

Fetch device-local history and print it to stdout.

```bash
cargo run --release -- c1plus history \
  --address A4:C1:38:C2:CC:F2 \
  --format csv
```

Supported history formats:

- `text`: human-readable summary plus tab-separated records
- `json`: one JSON object containing session metadata and all records
- `jsonl`: one JSON object per history record
- `csv`: CSV rows written to stdout

## Current reverse-engineering boundary

For `C1+`, the local BLE history we confirmed is device-local onboard history, not necessarily the same as the much longer phone-app chart history.

The app appears to merge BLE-fetched records with backend API data for longer time windows.

So:

- `history` in this CLI fetches what the sensor itself reports over BLE
- long-range chart history may later require a separate API integration path

## Known `C1+` packet coverage

Confirmed:

- params response
- current measurement response
- realtime notification/current frame parsing
- local history metadata and chunk retrieval

Still intentionally conservative:

- unknown bytes inside some `C1+` history records are intentionally omitted from CLI output
- no cloud/API path has been implemented yet

## Example workflow

1. Find the device:

```bash
cargo run --release -- scan
```

2. Read a current sample:

```bash
cargo run --release -- c1plus current --address <mac> --format json
```

## Contributing

Contributions are welcome, especially in these areas:

- support for additional Temtop sensor models
- better parsing coverage for currently unknown packet fields
- exporter or metrics integration paths built on top of the existing JSON and JSONL outputs
- platform-specific BLE testing and fixes

If you are adding support for a new model:

1. add a new sensor module under `src/sensor/`
2. add a sensor-specific CLI module under `src/cli/` when the command surface differs
3. keep transport and frame handling in the shared layers unless the behavior is genuinely model-specific
4. add sample-based tests for packet parsing

Before opening a pull request, run:

```bash
cargo test
```

or, if you want to use the same environment as CI:

```bash
nix develop --command cargo test --all-targets
```

GitHub Actions also runs the test suite and builds the binary artifact on pushes and pull requests.

Tag pushes like `v0.1.0` also publish the Linux binary to a GitHub release so the build is visible under Releases instead of only ephemeral workflow artifacts.

3. Fetch local history:

```bash
cargo run --release -- history --address <mac>
```

4. Run long-lived live collection later:

```bash
cargo run --release -- live --address <mac> --poll-secs 30 --format jsonl
```

That `live --format jsonl` path is the most likely future bridge into `vmagent` or a small local exporter.
