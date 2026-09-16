//! Controller-side sessions, observations, input authority, and visible GUI operations.

pub mod capture;
pub mod api;
pub mod auth;
pub mod screen;
pub mod subscriptions;
pub mod input;
pub mod control;
pub(crate) mod decoder;
pub mod error;
pub mod frames;
pub mod gui;
pub mod sessions;
pub mod terminals;
pub mod wire;

pub mod files;
pub mod text_clipboard;

pub mod file_clipboard;

pub mod displays;

pub mod views;

pub mod desktop;
pub mod security;

pub mod chat;
pub mod recording;

pub mod tunnels;
