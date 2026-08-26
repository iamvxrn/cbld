#!/usr/bin/env sh
set -e

BINARY="cbld"
INSTALL_DIR="${HOME}/.local/bin"

if [ -f "${INSTALL_DIR}/${BINARY}" ]; then
    rm -f "${INSTALL_DIR}/${BINARY}"
    echo "${BINARY} removed from ${INSTALL_DIR}"
else
    echo "${BINARY} is not installed in ${INSTALL_DIR}"
fi
