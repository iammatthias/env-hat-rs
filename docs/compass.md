# How to measure the compass's hard-iron offsets

`Motion::heading_deg` stays `None` until you pass the hard-iron offsets as
`Config::hard_iron_ut`. Steel and magnets on the board and the Pi add a
fixed field to every reading. The offsets are that field, in µT, on each
axis.

This is the usual min-max method. It has not been checked on this board.
To measure the offsets, follow these steps:

1. Run a loop that prints `motion.mag_ut` once a second. The
   `examples/soak.rs` loop is a starting point.
2. Away from steel and magnets, turn the board slowly through every
   orientation for about a minute.
3. For each axis, note the smallest and the largest value. The offset is
   their midpoint, (min + max) / 2.
4. Pass the three midpoints to `Hat::start`:

   ```rust,ignore
   let config = Config { hard_iron_ut: Some([x, y, z]), ..Config::default() };
   ```

After calibration, check that the field magnitude stays about the same in
every orientation. The Earth's field is 25 to 65 µT, depending on where
you are.
