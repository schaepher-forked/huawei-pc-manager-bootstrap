#!/bin/sh

set -e

cargo build --release -p version --target=x86_64-pc-windows-msvc
cargo build --release -p huawei-pc-manager-bootstrap-core -p huawei-pc-manager-bootstrap --target=i686-pc-windows-msvc

cp target/i686-pc-windows-msvc/release/huawei_pc_manager_bootstrap_core.dll dist
cp target/i686-pc-windows-msvc/release/huawei-pc-manager-bootstrap.exe dist