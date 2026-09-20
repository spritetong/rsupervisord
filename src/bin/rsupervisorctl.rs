// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rsupervisord::cli::run().await
}
