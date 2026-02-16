#!/bin/bash

set -e

DIRS=(
  "x86_64-pc-windows-msvc"
  "x86_64-unknown-linux-musl"
  "x86_64-apple-darwin"
  "aarch64-apple-darwin"
)

ORIGINAL_DIR=$(pwd)
ARTEFACT_NAME="pre-compiled.tar"

for DIR in "${DIRS[@]}"; do
  echo "Processing ${DIR}"

  cd "${DIR}"
  find . -type f -not -name "acquire-builder-*" -exec tar -rf ../${ARTEFACT_NAME} {} \;

  cd "${ORIGINAL_DIR}"
done
