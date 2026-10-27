# Record readings

`env-hat` doesn't store readings. It gives each value a fixed name and
unit, and you write them to the database you already use: InfluxDB,
Prometheus, TimescaleDB, SQLite, or a CSV file.

`Reading::values()` returns every present value paired with its
`env_hat::schema::Field`. Each field has a `name` and a `unit`. Record each
value under its name, with `Reading::unix_ms` as the timestamp. The order
is the order of the schema table on this page. To list the fields without
a reading, use `env_hat::schema::ALL`.

## The schema

A name is ASCII snake_case: the group, the quantity, and the unit. Names
never change once published, so recorded history stays continuous. A
field is missing from a reading when its chip hasn't sampled yet, is
absent, or failed. The last column lists the other times a field is
missing.

| Name | Unit | Source | Also missing when |
|---|---|---|---|
| `air_temperature_c` | °C | `Air::temperature_c` | |
| `air_humidity_pct` | % | `Air::humidity_pct` | |
| `air_pressure_hpa` | hPa | `Air::pressure_hpa` | |
| `air_dew_point_c` | °C | `Air::dew_point_c` | |
| `air_sea_level_pressure_hpa` | hPa | `Air::sea_level_hpa` | `Config::altitude_m` isn't set |
| `light_lux` | lx | `Light::lux` | |
| `light_full_counts` | counts | `Light::full` | |
| `light_ir_counts` | counts | `Light::ir` | |
| `light_gain` | times | `Light::gain` | |
| `uv_index` | UV index | `Uv::index` | |
| `uv_counts` | counts | `Uv::counts` | |
| `uv_ambient_lux` | lx | `Uv::ambient_lux` | the ambient-light conversion timed out |
| `gas_voc_index` | VOC Index | `Gas::voc_index` | the algorithm is in its first 46 samples |
| `gas_raw` | counts | `Gas::raw` | |
| `motion_roll_deg` | ° | `Motion::roll_deg` | |
| `motion_pitch_deg` | ° | `Motion::pitch_deg` | |
| `motion_heading_deg` | ° | `Motion::heading_deg` | `Config::hard_iron_ut` isn't set, or the magnetometer field is missing |
| `motion_accel_x_g` | g | `Motion::accel_g[0]` | |
| `motion_accel_y_g` | g | `Motion::accel_g[1]` | |
| `motion_accel_z_g` | g | `Motion::accel_g[2]` | |
| `motion_gyro_x_dps` | °/s | `Motion::gyro_dps[0]` | |
| `motion_gyro_y_dps` | °/s | `Motion::gyro_dps[1]` | |
| `motion_gyro_z_dps` | °/s | `Motion::gyro_dps[2]` | |
| `motion_mag_x_ut` | µT | `Motion::mag_ut[0]` | the field overflows, or the magnetometer is stalled or not started |
| `motion_mag_y_ut` | µT | `Motion::mag_ut[1]` | the field overflows, or the magnetometer is stalled or not started |
| `motion_mag_z_ut` | µT | `Motion::mag_ut[2]` | the field overflows, or the magnetometer is stalled or not started |
| `motion_temperature_c` | °C | `Motion::temperature_c` | |

For what each value means and how it is computed, see
[the `Reading` reference](api.md#reading) and
[how each value is computed](accuracy.md).

## Examples

Each example takes one `reading` from `Hat::every` and writes it.

To write a CSV row, put the names in the header once. Leave a cell empty
when its field is missing:

```rust,ignore
use env_hat::schema::ALL;

let header: Vec<&str> = ALL.iter().map(|f| f.name).collect();
println!("unix_ms,{}", header.join(","));

let values = reading.values();
let cells: Vec<String> = ALL
    .iter()
    .map(|f| values.iter().find(|(g, _)| g == f).map_or(String::new(), |(_, v)| v.to_string()))
    .collect();
println!("{},{}", reading.unix_ms, cells.join(","));
```

To write InfluxDB line protocol, with the timestamp in milliseconds:

```rust,ignore
let fields: Vec<String> = reading.values().iter().map(|(f, v)| format!("{}={v}", f.name)).collect();
println!("env_hat {} {}", fields.join(","), reading.unix_ms);
```

To write one row per value into a narrow SQL table:

```sql
CREATE TABLE readings (unix_ms INTEGER, name TEXT, value REAL);
```

```rust,ignore
for (field, value) in reading.values() {
    // Bind these to: INSERT INTO readings VALUES (?, ?, ?)
    let row = (reading.unix_ms, field.name, value);
}
```

To export the values to Prometheus, serve each value as a gauge named
after its field. Prometheus adds the timestamp when it scrapes.
