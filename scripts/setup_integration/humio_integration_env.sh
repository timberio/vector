#!/usr/bin/env bash
set -o pipefail

# humio_integration_env.sh
#
# SUMMARY
#
#   Builds and pulls down the Vector Humio Integration test environment

# Echo usage if something isn't right.
usage() {
    echo "Usage: $0 [-a Action to run {stop|start} ] [-t The container tool to use {docker|pdoman} ]  [-t The container enclosure to use {pod|network} ]" 1>&2; exit 1;
}

while getopts a:t:e: flag
do
    case "${flag}" in
        a) ACTION=${OPTARG}
          [[ ${ACTION} == "start" || ${ACTION} == "stop" ]] && usage;;
        t) CONTAINER_TOOL=${OPTARG}
          [[ ${CONTAINER_TOOL} == "podman" || ${CONTAINER_TOOL} == "docker" ]] && usage;;
        e) CONTAINER_ENCLOSURE=${OPTARG}
         [[ ${CONTAINER_ENCLOSURE} == "pod" || ${CONTAINER_ENCLOSURE} == "network" ]] && usage;;
        :)
         echo "ERROR: Option -$OPTARG requires an argument" usage
         ;;
        *)
          echo "ERROR: Invalid option -$OPTARG"
          usage
          ;;
    esac
done
shift $((OPTIND-1))
# Check required switches exist
if [ -z "${ACTION}" ] || [ -z "${CONTAINER_TOOL}" ] || [ -z "${CONTAINER_ENCLOSURE}" ]; then
    usage
fi

ACTION="${action:-"stop"}"
CONTAINER_TOOL="${tool:-"podman"}"
CONTAINER_ENCLOSURE="${enclosure:-"pod"}"

#
# Functions
#

start_podman () {
	"${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" create --replace --name vector-test-integration-humio -p 8080:8080
	"${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-humio --name vector_humio humio/humio:1.13.1
}

start_docker () {
	"${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" create vector-test-integration-humio
	"${CONTAINER_TOOL}" run -d --"${CONTAINER_ENCLOSURE}"=vector-test-integration-humio -p 8080:8080 --name vector_humio humio/humio:1.13.1
}

stop_podman () {
	"${CONTAINER_TOOL}" rm --force vector_humio 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" stop vector-test-integration-humio 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" rm --force vector-test-integration-humio 2>/dev/null; true
}

stop_docker () {
  "${CONTAINER_TOOL}" rm --force vector_humio 2>/dev/null; true
	"${CONTAINER_TOOL}" rm --force vector_humio 2>/dev/null; true
  "${CONTAINER_TOOL}" "${CONTAINER_ENCLOSURE}" rm vector-test-integration-humio 2>/dev/null; true
}

echo "Running $ACTION action for Humio integration tests environment"

"${ACTION}"_"${CONTAINER_TOOL}"
