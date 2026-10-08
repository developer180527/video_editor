//! Out-of-process add-ons (where `Capabilities::processes` is true).
//!
//! The host starts the add-on through the `ProcessHost` port and talks over
//! its pipes with length-prefixed messages; frames travel through shared
//! memory. A watchdog restarts an add-on that stops answering and disables one
//! that keeps crashing. Not implemented in Phase A.
