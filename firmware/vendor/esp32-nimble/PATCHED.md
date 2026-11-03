# esp32-nimble 0.13.0, patched

A copy of [esp32-nimble](https://github.com/taks/esp32-nimble) 0.13.0
(Apache-2.0, see LICENSE), used through `[patch.crates-io]` in
`firmware/Cargo.toml`. Only `src/client/ble_client.rs` differs:

- Every MTU event signalled the client's single completion signal. A peer
  that starts an MTU exchange of its own (JK BMS modules do) raises a second
  event, which ended the following wait early: service discovery came back
  with an empty list. `connect` now takes the first MTU event only
  (`awaiting_mtu`).
- A link lost before the MTU exchange finished left `connect` waiting
  forever; the disconnect now ends that wait with its reason.

Offered upstream: https://github.com/taks/esp32-nimble/pull/210. Once a
release has it, drop this copy and the `[patch.crates-io]` entry.

The examples and the packaged toolchain archive are left out.
