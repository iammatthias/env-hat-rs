# ADR 0003: a recording schema, not a store

Status: accepted, 2026-10-27. Supersedes [ADR 0001](0001-tsdb-storage.md).

## Problem

The repository shipped `tsdb`, a time-series store built for this HAT on
an SD card. Anyone recording the HAT's readings already runs a database
they chose for their own reasons, such as InfluxDB, Prometheus,
TimescaleDB, or SQLite. A store in this crate either duplicates that
database or competes with it. It also tied the HAT drivers to one storage
format.

## Decision

Remove `tsdb`. Give every value a fixed name and unit instead, and let
callers write them anywhere.

- `env_hat::schema::Field { name, unit }`, one constant per value, and
  `schema::ALL` in a fixed order.
- `Reading::values() -> Vec<(Field, f64)>`, every present value with its
  field. A missing group or value yields nothing.
- No encoder, no database types, and no new dependency.

A name is ASCII snake_case: the group, the quantity, and the unit, as in
`air_temperature_c`. It matches the struct field it comes from, and it is
valid as a Prometheus metric name, an InfluxDB field key, and a SQL column.
A test pins every name and unit, and another checks that the
[recording guide](../recording.md) lists exactly the fields in `ALL`.

## Alternatives considered

| Alternative | Why it lost |
|---|---|
| Keep `tsdb` | It solves storage for one deployment. Everyone else already has a database. |
| Ship encoders for InfluxDB, Prometheus, and SQL | Each encoder is a format to maintain and a choice the caller can make in a few lines. The [recording guide](../recording.md) shows those lines. |
| Derive `serde::Serialize` on `Reading` | It adds a dependency, and its nested shape doesn't map to flat time-series fields. |
| Prometheus base units (`air_temperature_celsius`) | Names that match the struct fields are easier to trace. Prometheus accepts these names as they are. |

## Consequences

- Renaming a field is a breaking change, because databases hold history
  under the old name. New fields can be added.
- An application with its own recorded history keeps its own names. It
  can map from these fields, or record both.
- `env-hat` still depends on `libc` only.
