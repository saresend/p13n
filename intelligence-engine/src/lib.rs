//! Personalization server: collects the actions users take in an app and streams back
//! suggestions for what to do next.

pub mod action;
pub mod api;
pub mod db;
pub mod engine;
pub mod model;
pub mod queue;
pub mod vocabulary;
