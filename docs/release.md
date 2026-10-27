# Release checklist

This page lists what must happen before `env-hat` is published. Work
through the sections in order, because later steps depend on earlier
ones.

## Decision for the owner

The vendor port, the only code here derived from Waveshare's unlicensed
demo, was deleted. Confirm that nothing else in the tree derives from that
demo. The drivers cite the chip datasheets, and the TSL2591 lux uses
Adafruit's published constants.

## Package

1. Remove `publish = false` from the workspace manifest.
2. Run `cargo package -p env-hat --list` and check the contents. It must
   include `README.md`.
3. Add a `CHANGELOG.md` with the 0.1.0 entry.
4. In the README, replace the git dependency with the crates.io version.

## Publish

1. Run `bash ci/run.sh` on the release commit.
2. Tag the commit `v0.1.0`.
3. Run `cargo publish -p env-hat`. The name `env-hat` was free on
   2026-10-27.

## Known limits to state in the release notes

- The compass heading is not verified on a device. For the calibration
  steps, see [how to measure the hard-iron offsets](compass.md).
- The lux is uncalibrated. It uses Adafruit's community constants.
- The temperature is the board's, which the Pi warms. To set the offset,
  see [how to set the temperature offset](api.md#config).
