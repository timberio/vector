#!/usr/bin/env bash

# test-integration-docker.sh
#
# SUMMARY
#
#   Run integration tests for Docker components only.

cargo test --no-default-features --features docker-integration-tests
