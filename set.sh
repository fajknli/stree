#!/bin/sh

# Author:       fajknli
# Email:        fajknli@gmail.com
# Created Time: 2026-08-08 15:39


cargo build --release && cp target/release/stree $HOME/.local/bin/
