// SPDX-License-Identifier: LGPL-3.0-only
// Copyright (c) 2024 Shane Utt

//! Listener validation rules.

mod address;
mod rules;
mod timeouts;

pub(in crate::config::validate) use rules::{validate_listener_names, validate_listeners};
