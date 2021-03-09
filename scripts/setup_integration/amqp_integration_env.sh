#!/usr/bin/env bash
set -o pipefail

# amqp_integration_env.sh
#
# SUMMARY
#
#   Builds and pulls down the Vector Amqp Integration test environment

if [ $# -ne 1 ]
then
    echo "Usage: $0 {stop|start}" 1>&2; exit 1;
    exit 1
fi
ACTION=$1

#
# Functions
#

start_podman () {
  podman pod create --replace --name vector-test-integration-amqp -p 5672:5672
  podman run -d --pod=vector-test-integration-amqp --name vector_amqp rabbitmq:3.8
}

start_docker () {
  docker network create vector-test-integration-amqp
  docker run -d --network=vector-test-integration-amqp -p 5672:5672 --name vector_amqp rabbitmq:3.8
}

stop_podman () {
  podman pod stop vector-test-integration-amqp 2>/dev/null; true
  podman pod rm --force vector-test-integration-amqp 2>/dev/null; true
}

stop_docker () {
  docker rm --force vector_amqp vector_amqp 2>/dev/null; true
  docker network rm vector-test-integration-amqp 2>/dev/null; true
}

echo "Running $ACTION action for Amqp integration tests environment"

"${ACTION}"_"${CONTAINER_TOOL}"
