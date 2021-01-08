#!/usr/bin/env bash
set -o pipefail

# kafka_integration_env.sh
#
# SUMMARY
#
#   Builds and pulls down the Vector Kafka Integration test environment

if [ $# -ne 2 ]
then
    echo "Usage: $0 {stop|start} {docker|podman}" 1>&2; exit 1;
    exit 1
fi
ACTION=$1
CONTAINER_TOOL=$2
#
# Functions
#

start_podman () {
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" create --replace --name vector-test-integration-kafka -p 2181:2181 -p 9091-9093:9091-9093
  "${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-kafka --name vector_zookeeper wurstmeister/zookeeper
  "${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-kafka -e KAFKA_BROKER_ID=1 \
	 -e KAFKA_ZOOKEEPER_CONNECT=vector_zookeeper:2181 -e KAFKA_LISTENERS=PLAINTEXT://:9091,SSL://:9092,SASL_PLAINTEXT://:9093 \
	 -e KAFKA_ADVERTISED_LISTENERS=PLAINTEXT://localhost:9091,SSL://localhost:9092,SASL_PLAINTEXT://localhost:9093 \
	 -e KAFKA_SSL_KEYSTORE_LOCATION=/certs/localhost.p12 -e KAFKA_SSL_KEYSTORE_PASSWORD=NOPASS \
	 -e KAFKA_SSL_TRUSTSTORE_LOCATION=/certs/localhost.p12 -e KAFKA_SSL_TRUSTSTORE_PASSWORD=NOPASS \
	 -e KAFKA_SSL_KEY_PASSWORD=NOPASS -e KAFKA_SSL_ENDPOINT_IDENTIFICATION_ALGORITHM=none \
	 -e KAFKA_OPTS="-Djava.security.auth.login.config=/etc/kafka/kafka_server_jaas.conf" \
	 -e KAFKA_INTER_BROKER_LISTENER_NAME=SASL_PLAINTEXT -e KAFKA_SASL_ENABLED_MECHANISMS=PLAIN \
	 -e KAFKA_SASL_MECHANISM_INTER_BROKER_PROTOCOL=PLAIN -v "$(pwd)"/tests/data/localhost.p12:/certs/localhost.p12:ro \
	 -v "$(pwd)"/tests/data/kafka_server_jaas.conf:/etc/kafka/kafka_server_jaas.conf --name vector_kafka wurstmeister/kafka
}

start_docker () {
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" create vector-test-integration-kafka
  "${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-kafka -p 2181:2181 --name vector_zookeeper wurstmeister/zookeeper
  "${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-kafka -p 9091-9093:9091-9093 -e KAFKA_BROKER_ID=1 \
	 -e KAFKA_ZOOKEEPER_CONNECT=vector_zookeeper:2181 -e KAFKA_LISTENERS=PLAINTEXT://:9091,SSL://:9092,SASL_PLAINTEXT://:9093 \
	 -e KAFKA_ADVERTISED_LISTENERS=PLAINTEXT://localhost:9091,SSL://localhost:9092,SASL_PLAINTEXT://localhost:9093 \
	 -e KAFKA_SSL_KEYSTORE_LOCATION=/certs/localhost.p12 -e KAFKA_SSL_KEYSTORE_PASSWORD=NOPASS \
	 -e KAFKA_SSL_TRUSTSTORE_LOCATION=/certs/localhost.p12 -e KAFKA_SSL_TRUSTSTORE_PASSWORD=NOPASS \
	 -e KAFKA_SSL_KEY_PASSWORD=NOPASS -e KAFKA_SSL_ENDPOINT_IDENTIFICATION_ALGORITHM=none \
	 -e KAFKA_OPTS="-Djava.security.auth.login.config=/etc/kafka/kafka_server_jaas.conf" \
	 -e KAFKA_INTER_BROKER_LISTENER_NAME=SASL_PLAINTEXT -e KAFKA_SASL_ENABLED_MECHANISMS=PLAIN \
	 -e KAFKA_SASL_MECHANISM_INTER_BROKER_PROTOCOL=PLAIN -v "$(pwd)"/tests/data/localhost.p12:/certs/localhost.p12:ro \
	 -v "$(pwd)"/tests/data/kafka_server_jaas.conf:/etc/kafka/kafka_server_jaas.conf --name vector_kafka wurstmeister/kafka
}

stop_podman () {
  "${CONTAINER_TOOL}" rm --force vector_kafka vector_zookeeper 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" stop vector-test-integration-kafka 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" rm --force vector-test-integration-kafka 2>/dev/null; true
}

stop_docker () {
  "${CONTAINER_TOOL}" rm --force vector_kafka vector_zookeeper 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" rm vector-test-integration-kafka 2>/dev/null; true
}

echo "Running $ACTION action for Kafka integration tests environment"

"${ACTION}"_"${CONTAINER_TOOL}"
