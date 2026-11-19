#![recursion_limit = "512"]

mod app;
mod cli;
mod library;
mod models;
mod net;
mod storage;
mod terminal;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use crate::{app::App, cli::Args, storage::Store};

fn main() -> Result<()> {
    let args = Args::parse();
    let data_dir = args.data_dir.unwrap_or_else(|| {
        directories::ProjectDirs::from("", "", "fanqie")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".fanqie"))
    });
    let store = Store::new(data_dir)?;
    let mut app = App::new(store)?;
    // CLI overrides are session-only, so shell-provided tokens are not persisted.
    if let Some(api) = args.content_api {
        app.settings.content_api = api;
    }
    if let Some(api) = args.search_api {
        app.settings.search_api = api;
    }
    if args.demo {
        app.demo();
    } else if let Some(path) = args.import {
        app.import(path);
    } else if let Some(book) = args.book {
        app.open_remote(book);
    } else if let Some(query) = args.search {
        app.execute(&format!("/search {query}"));
    }

    terminal::run(&mut app)
}
