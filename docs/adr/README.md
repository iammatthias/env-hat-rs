# Architecture decisions

Each record gives one decision: the problem, the choice, and the
alternatives it beat.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-tsdb-storage.md) | The `tsdb` store is one append-only log per UTC day, not SQLite. | Superseded by 0003 |
| [0002](0002-hat-api.md) | One `Hat` type with a sampler thread per chip, and an oracle for every value. | Accepted, amended |
| [0003](0003-schema-not-store.md) | A fixed name and unit for every value instead of a store, so callers record to any database. | Accepted |
