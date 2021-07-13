# .PHONY: $(MAKECMDGOALS) all
.DEFAULT_GOAL := help

mkfile_path := $(abspath $(lastword $(MAKEFILE_LIST)))
mkfile_dir := $(dir $(mkfile_path))

# Begin OS detection
ifeq ($(OS),Windows_NT) # is Windows_NT on XP, 2000, 7, Vista, 10...
    export OPERATING_SYSTEM := Windows
	export RUST_TARGET ?= "x86_64-unknown-windows-msvc"
    export DEFAULT_FEATURES = default-msvc
	undefine DNSTAP_BENCHES
else
    export OPERATING_SYSTEM := $(shell uname)  # same as "uname -s"
	export RUST_TARGET ?= "x86_64-unknown-linux-gnu"
    export DEFAULT_FEATURES = default
	export DNSTAP_BENCHES := dnstap-benches
endif

# Override this with any scopes for testing/benching.
export SCOPE ?=
# Override this with any extra flags for cargo bench
export CARGO_BENCH_FLAGS ?=
# override this to put criterion output elsewhere
export CRITERION_HOME ?= $(mkfile_dir)target/criterion
# Override to false to disable autospawning services on integration tests.
export AUTOSPAWN ?= true
# Override to control if services are turned off after integration tests.
export AUTODESPAWN ?= ${AUTOSPAWN}
# Override autoinstalling of tools. (Eg `cargo install`)
export AUTOINSTALL ?= false
# Override to true for a bit more log output in your environment building (more coming!)
export VERBOSE ?= false
# Override to set a different Rust toolchain
export RUST_TOOLCHAIN ?= $(shell cat rust-toolchain)
# Override the container tool. Tries docker first and then tries podman.
export CONTAINER_TOOL ?= auto
ifeq ($(CONTAINER_TOOL),auto)
	override CONTAINER_TOOL = $(shell docker version >/dev/null 2>&1 && echo docker || echo podman)
endif
# If we're using podman create pods else if we're using docker create networks.
export CURRENT_DIR = $(shell pwd)

# Override this to automatically enter a container containing the correct, full, official build environment for Vector, ready for development
export ENVIRONMENT ?= false
# The upstream container we publish artifacts to on a successful master build.
export ENVIRONMENT_UPSTREAM ?= timberio/ci_image
# Override to disable building the container, having it pull from the Github packages repo instead
# TODO: Disable this by default. Blocked by `docker pull` from Github Packages requiring authenticated login
export ENVIRONMENT_AUTOBUILD ?= true
# Override this when appropriate to disable a TTY being available in commands with `ENVIRONMENT=true`
export ENVIRONMENT_TTY ?= true
# A list of WASM modules by name
export WASM_MODULES = $(patsubst tests/data/wasm/%/,%,$(wildcard tests/data/wasm/*/))
# The same WASM modules, by output path.
export WASM_MODULE_OUTPUTS = $(patsubst %,/target/wasm32-wasi/%,$(WASM_MODULES))

# Set dummy AWS credentials if not present - used for AWS and ES integration tests
export AWS_ACCESS_KEY_ID ?= "dummy"
export AWS_SECRET_ACCESS_KEY ?= "dummy"

# Set version
export VERSION ?= $(shell scripts/version.sh)

# Set if you are on the CI and actually want the things to happen. (Non-CI users should never set this.)
export CI ?= false

FORMATTING_BEGIN_YELLOW = \033[0;33m
FORMATTING_BEGIN_BLUE = \033[36m
FORMATTING_END = \033[0m

# "One weird trick!" https://www.gnu.org/software/make/manual/make.html#Syntax-of-Functions
EMPTY:=
SPACE:= ${EMPTY} ${EMPTY}

help:
	@printf -- "${FORMATTING_BEGIN_BLUE}                                      __   __  __${FORMATTING_END}\n"
	@printf -- "${FORMATTING_BEGIN_BLUE}                                      \ \ / / / /${FORMATTING_END}\n"
	@printf -- "${FORMATTING_BEGIN_BLUE}                                       \ V / / / ${FORMATTING_END}\n"
	@printf -- "${FORMATTING_BEGIN_BLUE}                                        \_/  \/  ${FORMATTING_END}\n"
	@printf -- "\n"
	@printf -- "                                      V E C T O R\n"
	@printf -- "\n"
	@printf -- "---------------------------------------------------------------------------------------\n"
	@printf -- "Want to use ${FORMATTING_BEGIN_YELLOW}\`docker\`${FORMATTING_END} or ${FORMATTING_BEGIN_YELLOW}\`podman\`${FORMATTING_END}? See ${FORMATTING_BEGIN_YELLOW}\`ENVIRONMENT=true\`${FORMATTING_END} commands. (Default ${FORMATTING_BEGIN_YELLOW}\`CONTAINER_TOOL=docker\`${FORMATTING_END})\n"
	@printf -- "\n"
	@awk 'BEGIN {FS = ":.*##"; printf "Usage: make ${FORMATTING_BEGIN_BLUE}<target>${FORMATTING_END}\n"} /^[a-zA-Z0-9_-]+:.*?##/ { printf "  ${FORMATTING_BEGIN_BLUE}%-46s${FORMATTING_END} %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Environment

# These are some predefined macros, please use them!
ifeq ($(ENVIRONMENT), true)
define MAYBE_ENVIRONMENT_EXEC
${ENVIRONMENT_EXEC}
endef
else
define MAYBE_ENVIRONMENT_EXEC

endef
endif

ifeq ($(ENVIRONMENT), true)
define MAYBE_ENVIRONMENT_COPY_ARTIFACTS
${ENVIRONMENT_COPY_ARTIFACTS}
endef
else
define MAYBE_ENVIRONMENT_COPY_ARTIFACTS

endef
endif

# We use a volume here as non-Linux hosts are extremely slow to share disks, and Linux hosts tend to get permissions clobbered.
define ENVIRONMENT_EXEC
	${ENVIRONMENT_PREPARE}
	@echo "Entering environment..."
	@mkdir -p target
	$(CONTAINER_TOOL) run \
			--name vector-environment \
			--rm \
			$(if $(findstring true,$(ENVIRONMENT_TTY)),--tty,) \
			--init \
			--interactive \
			--env INSIDE_ENVIRONMENT=true \
			--network host \
			--mount type=bind,source=${CURRENT_DIR},target=/git/timberio/vector \
			--mount type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock \
			--mount type=volume,source=vector-target,target=/git/timberio/vector/target \
			--mount type=volume,source=vector-cargo-cache,target=/root/.cargo \
			$(ENVIRONMENT_UPSTREAM)
endef


ifeq ($(ENVIRONMENT_AUTOBUILD), true)
define ENVIRONMENT_PREPARE
	@echo "Building the environment. (ENVIRONMENT_AUTOBUILD=true) This may take a few minutes..."
	$(CONTAINER_TOOL) build \
		$(if $(findstring true,$(VERBOSE)),,--quiet) \
		--tag $(ENVIRONMENT_UPSTREAM) \
		--file scripts/environment/Dockerfile \
		.
endef
else
define ENVIRONMENT_PREPARE
	$(CONTAINER_TOOL) pull $(ENVIRONMENT_UPSTREAM)
endef
endif

.PHONY: check-container-tool
check-container-tool: ## Checks what container tool is installed
	@echo -n "Checking if $(CONTAINER_TOOL) is available..." && \
	$(CONTAINER_TOOL) version 1>/dev/null && echo "yes"

.PHONY: environment
environment: export ENVIRONMENT_TTY = true ## Enter a full Vector dev shell in $CONTAINER_TOOL, binding this folder to the container.
environment:
	${ENVIRONMENT_EXEC}

.PHONY: environment-prepare
environment-prepare: ## Prepare the Vector dev shell using $CONTAINER_TOOL.
	${ENVIRONMENT_PREPARE}

.PHONY: environment-clean
environment-clean: ## Clean the Vector dev shell using $CONTAINER_TOOL.
	@$(CONTAINER_TOOL) volume rm -f vector-target vector-cargo-cache
	@$(CONTAINER_TOOL) rmi $(ENVIRONMENT_UPSTREAM) || true

.PHONY: environment-push
environment-push: environment-prepare ## Publish a new version of the container image.
	$(CONTAINER_TOOL) push $(ENVIRONMENT_UPSTREAM)

##@ Building
.PHONY: build
build: check-build-tools
build: export CFLAGS += -g0 -O3
build: ## Build the project in release mode (Supports `ENVIRONMENT=true`)
	${MAYBE_ENVIRONMENT_EXEC} cargo build --release --no-default-features --features ${DEFAULT_FEATURES}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: build-dev
build-dev: ## Build the project in development mode (Supports `ENVIRONMENT=true`)
	${MAYBE_ENVIRONMENT_EXEC} cargo build --no-default-features --features ${DEFAULT_FEATURES}

.PHONY: build-x86_64-unknown-linux-gnu
build-x86_64-unknown-linux-gnu: target/x86_64-unknown-linux-gnu/release/vector ## Build a release binary for the x86_64-unknown-linux-gnu triple.
	@echo "Output to ${<}"

.PHONY: build-aarch64-unknown-linux-gnu
build-aarch64-unknown-linux-gnu: target/aarch64-unknown-linux-gnu/release/vector ## Build a release binary for the aarch64-unknown-linux-gnu triple.
	@echo "Output to ${<}"

.PHONY: build-x86_64-unknown-linux-musl
build-x86_64-unknown-linux-musl: target/x86_64-unknown-linux-musl/release/vector ## Build a release binary for the x86_64-unknown-linux-musl triple.
	@echo "Output to ${<}"

.PHONY: build-aarch64-unknown-linux-musl
build-aarch64-unknown-linux-musl: target/aarch64-unknown-linux-musl/release/vector ## Build a release binary for the aarch64-unknown-linux-musl triple.
	@echo "Output to ${<}"

.PHONY: build-armv7-unknown-linux-gnueabihf
build-armv7-unknown-linux-gnueabihf: target/armv7-unknown-linux-gnueabihf/release/vector ## Build a release binary for the armv7-unknown-linux-gnueabihf triple.
	@echo "Output to ${<}"

.PHONY: build-armv7-unknown-linux-musleabihf
build-armv7-unknown-linux-musleabihf: target/armv7-unknown-linux-musleabihf/release/vector ## Build a release binary for the armv7-unknown-linux-musleabihf triple.
	@echo "Output to ${<}"

.PHONY: build-graphql-schema
build-graphql-schema: ## Generate the `schema.json` for Vector's GraphQL API
	${MAYBE_ENVIRONMENT_EXEC} cargo run --bin graphql-schema --no-default-features --features=default-no-api-client

.PHONY: check-build-tools
check-build-tools:
ifeq (, $(shell which rustc))
	$(error "Please install Rust: https://www.rust-lang.org/tools/install")
endif
ifeq (, $(shell which cue))
	$(error "Please install Cue: https://cuelang.org/docs/install/")
endif

##@ Cross Compiling
.PHONY: cross-enable
cross-enable: cargo-install-cross

.PHONY: CARGO_HANDLES_FRESHNESS
CARGO_HANDLES_FRESHNESS:
	${EMPTY}

# GNU Make < 3.82 pattern matching priority depends on the definition order
# so cross-image-% must be defined before cross-%
.PHONY: cross-image-%
cross-image-%: export TRIPLE =$($(strip @):cross-image-%=%)
cross-image-%:
	$(CONTAINER_TOOL) build \
		--tag vector-cross-env:${TRIPLE} \
		--file scripts/cross/${TRIPLE}.dockerfile \
		scripts/cross

# This is basically a shorthand for folks.
# `cross-anything-triple` will call `cross anything --target triple` with the right features.
.PHONY: cross-%
cross-%: export PAIR =$(subst -, ,$($(strip @):cross-%=%))
cross-%: export COMMAND ?=$(word 1,${PAIR})
cross-%: export TRIPLE ?=$(subst ${SPACE},-,$(wordlist 2,99,${PAIR}))
cross-%: export PROFILE ?= release
cross-%: export CFLAGS += -g0 -O3
cross-%: cargo-install-cross
	$(MAKE) -k cross-image-${TRIPLE}
	cross ${COMMAND} \
		$(if $(findstring release,$(PROFILE)),--release,) \
		--target ${TRIPLE} \
		--no-default-features \
		--features target-${TRIPLE}

target/%/vector: export PAIR =$(subst /, ,$(@:target/%/vector=%))
target/%/vector: export TRIPLE ?=$(word 1,${PAIR})
target/%/vector: export PROFILE ?=$(word 2,${PAIR})
target/%/vector: export CFLAGS += -g0 -O3
target/%/vector: cargo-install-cross CARGO_HANDLES_FRESHNESS
	$(MAKE) -k cross-image-${TRIPLE}
	cross build \
		$(if $(findstring release,$(PROFILE)),--release,) \
		--target ${TRIPLE} \
		--no-default-features \
		--features target-${TRIPLE}

target/%/vector.tar.gz: export PAIR =$(subst /, ,$(@:target/%/vector.tar.gz=%))
target/%/vector.tar.gz: export TRIPLE ?=$(word 1,${PAIR})
target/%/vector.tar.gz: export PROFILE ?=$(word 2,${PAIR})
target/%/vector.tar.gz: target/%/vector CARGO_HANDLES_FRESHNESS
	rm -rf target/scratch/vector-${TRIPLE} || true
	mkdir -p target/scratch/vector-${TRIPLE}/bin target/scratch/vector-${TRIPLE}/etc
	cp --recursive --force --verbose \
		target/${TRIPLE}/${PROFILE}/vector \
		target/scratch/vector-${TRIPLE}/bin/vector
	cp --recursive --force --verbose \
		README.md \
		LICENSE \
		config \
		target/scratch/vector-${TRIPLE}/
	cp --recursive --force --verbose \
		distribution/systemd \
		target/scratch/vector-${TRIPLE}/etc/
	tar --create \
		--gzip \
		--verbose \
		--file target/${TRIPLE}/${PROFILE}/vector.tar.gz \
		--directory target/scratch/ \
		./vector-${TRIPLE}
	rm -rf target/scratch/

##@ Testing (Supports `ENVIRONMENT=true`)

.PHONY: test
test: ## Run the unit test suite
	${MAYBE_ENVIRONMENT_EXEC} cargo test --quiet --workspace --no-fail-fast --no-default-features --features "${DEFAULT_FEATURES} metrics-benches remap-benches statistic-benches ${DNSTAP_BENCHES} benches" ${SCOPE}

.PHONY: test-all
test-all: test test-behavior test-integration ## Runs all tests, unit, behaviorial, and integration.

.PHONY: test-x86_64-unknown-linux-gnu
test-x86_64-unknown-linux-gnu: cross-test-x86_64-unknown-linux-gnu ## Runs unit tests on the x86_64-unknown-linux-gnu triple
	${EMPTY}

.PHONY: test-aarch64-unknown-linux-gnu
test-aarch64-unknown-linux-gnu: cross-test-aarch64-unknown-linux-gnu ## Runs unit tests on the aarch64-unknown-linux-gnu triple
	${EMPTY}

.PHONY: test-behavior
test-behavior: ## Runs behaviorial test
	${MAYBE_ENVIRONMENT_EXEC} cargo run -- test tests/behavior/**/*

.PHONY: test-integration
test-integration: ## Runs all integration tests
test-integration: test-integration-aws test-integration-azure test-integration-clickhouse test-integration-docker-logs test-integration-elasticsearch
test-integration: test-integration-eventstoredb_metrics test-integration-fluent test-integration-gcp test-integration-humio test-integration-influxdb
test-integration: test-integration-kafka test-integration-logstash test-integration-loki test-integration-mongodb_metrics test-integration-nats
test-integration: test-integration-nginx test-integration-postgresql_metrics test-integration-prometheus test-integration-pulsar
test-integration: test-integration-redis test-integration-splunk test-integration-dnstap

.PHONY: test-integration-aws
test-integration-aws: ## Runs AWS integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh aws stop
	@scripts/setup_integration_env.sh aws start
	sleep 10 # Many services are very slow... Give them a sec...
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features aws-integration-tests --lib ::aws_
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh aws stop
endif

.PHONY: test-integration-azure
test-integration-azure: ## Runs Azure integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh azure stop
	@scripts/setup_integration_env.sh azure start
	sleep 5 # Many services are very slow... Give them a sec...
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features azure-integration-tests --lib ::azure_ -- --nocapture
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh azure stop
endif

.PHONY: test-integration-clickhouse
test-integration-clickhouse: ## Runs Clickhouse integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh clickhouse stop
	@scripts/setup_integration_env.sh clickhouse start
	sleep 5 # Many services are very slow... Give them a sec...
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features clickhouse-integration-tests --lib ::clickhouse::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh clickhouse stop
endif

.PHONY: test-integration-docker-logs
test-integration-docker-logs: ## Runs Docker Logs integration tests
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features docker-logs-integration-tests --lib ::docker_logs::

.PHONY: test-integration-elasticsearch
test-integration-elasticsearch: ## Runs Elasticsearch integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh elasticsearch stop
	@scripts/setup_integration_env.sh elasticsearch start
	sleep 60 # Many services are very slow... Give them a sec...
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features es-integration-tests --lib ::elasticsearch::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh elasticsearch stop
endif

.PHONY: test-integration-eventstoredb_metrics
test-integration-eventstoredb_metrics: ## Runs EventStoreDB metric integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh eventstoredb_metrics stop
	@scripts/setup_integration_env.sh eventstoredb_metrics start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features eventstoredb_metrics-integration-tests --lib ::eventstoredb_metrics:: -- --nocapture
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh eventstoredb_metrics stop
endif

.PHONY: test-integration-fluent
test-integration-fluent: ## Runs Fluent integration tests
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features fluent-integration-tests --lib ::fluent::

.PHONY: test-integration-gcp
test-integration-gcp: ## Runs GCP integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh gcp stop
	@scripts/setup_integration_env.sh gcp start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features "gcp-integration-tests gcp-pubsub-integration-tests gcp-cloud-storage-integration-tests" \
	 --lib ::gcp::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh gcp stop
endif

.PHONY: test-integration-humio
test-integration-humio: ## Runs Humio integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh humio stop
	@scripts/setup_integration_env.sh humio start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features humio-integration-tests --lib "::humio::.*::integration_tests::"
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh humio stop
endif
.PHONY: test-integration-influxdb
test-integration-influxdb: ## Runs InfluxDB integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh influxdb stop
	@scripts/setup_integration_env.sh influxdb start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features influxdb-integration-tests --lib integration_tests:: --  ::influxdb
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh influxdb stop
endif

.PHONY: test-integration-kafka
test-integration-kafka: ## Runs Kafka integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh kafka stop
	@scripts/setup_integration_env.sh kafka start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features "kafka-integration-tests rdkafka-plain" --lib ::kafka::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh kafka stop
endif

.PHONY: test-integration-loki
test-integration-loki: ## Runs Loki integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh loki stop
	@scripts/setup_integration_env.sh loki start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features loki-integration-tests --lib ::loki::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh loki stop
endif

.PHONY: test-integration-logstash
test-integration-logstash: ## Runs Logstash integration tests
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features logstash-integration-tests --lib ::logstash:: -- --nocapture

.PHONY: test-integration-mongodb_metrics
test-integration-mongodb_metrics: ## Runs MongoDB Metrics integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh mongodb_metrics stop
	@scripts/setup_integration_env.sh mongodb_metrics start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features mongodb_metrics-integration-tests --lib ::mongodb_metrics::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh mongodb_metrics stop
endif

.PHONY: test-integration-nats
test-integration-nats: ## Runs NATS integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh nats stop
	@scripts/setup_integration_env.sh nats start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features nats-integration-tests --lib ::nats::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh nats stop
endif

.PHONY: test-integration-nginx
test-integration-nginx: ## Runs nginx integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh nginx stop
	@scripts/setup_integration_env.sh nginx start
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features nginx-integration-tests --lib ::nginx_metrics::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh nginx stop
endif

.PHONY: test-integration-postgresql_metrics
test-integration-postgresql_metrics: ## Runs postgresql_metrics integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh postgresql_metrics stop
	@scripts/setup_integration_env.sh postgresql_metrics start
	sleep 5 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features postgresql_metrics-integration-tests --lib ::postgresql_metrics::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh postgresql_metrics stop
endif

.PHONY: test-integration-prometheus
test-integration-prometheus: ## Runs Prometheus integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh influxdb stop
	@scripts/setup_integration_env.sh prometheus stop
	@scripts/setup_integration_env.sh influxdb start
	@scripts/setup_integration_env.sh prometheus start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features prometheus-integration-tests --lib ::prometheus::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh influxdb stop
	@scripts/setup_integration_env.sh prometheus stop
endif

.PHONY: test-integration-pulsar
test-integration-pulsar: ## Runs Pulsar integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh pulsar stop
	@scripts/setup_integration_env.sh pulsar start
	sleep 15 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features pulsar-integration-tests --lib ::pulsar::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh pulsar stop
endif

.PHONY: test-integration-redis
test-integration-redis: ## Runs Redis integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh redis stop
	@scripts/setup_integration_env.sh redis start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features redis-integration-tests --lib ::redis:: -- --nocapture
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh redis stop
endif

.PHONY: test-integration-splunk
test-integration-splunk: ## Runs Splunk integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh splunk stop
	@scripts/setup_integration_env.sh splunk start
	sleep 10 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features splunk-integration-tests --lib ::splunk_hec::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh splunk stop
endif

.PHONY: test-integration-dnstap
test-integration-dnstap: ## Runs dnstap integration tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh dnstap stop
	@scripts/setup_integration_env.sh dnstap start
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features dnstap-integration-tests --lib ::dnstap::
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh dnstap stop
endif

.PHONY: test-e2e-kubernetes
test-e2e-kubernetes: ## Runs Kubernetes E2E tests (Sorry, no `ENVIRONMENT=true` support)
	@scripts/test-e2e-kubernetes.sh

.PHONY: test-shutdown
test-shutdown: ## Runs shutdown tests
ifeq ($(AUTOSPAWN), true)
	@scripts/setup_integration_env.sh kafka stop
	@scripts/setup_integration_env.sh kafka start
	sleep 30 # Many services are very slow... Give them a sec..
endif
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features shutdown-tests --test shutdown -- --test-threads 4
ifeq ($(AUTODESPAWN), true)
	@scripts/setup_integration_env.sh kafka stop
endif

.PHONY: test-cli
test-cli: ## Runs cli tests
	${MAYBE_ENVIRONMENT_EXEC} cargo test --no-fail-fast --no-default-features --features cli-tests --test cli -- --test-threads 4

.PHONY: test-wasm-build-modules
test-wasm-build-modules: $(WASM_MODULE_OUTPUTS) ### Build all WASM test modules

$(WASM_MODULE_OUTPUTS): MODULE = $(notdir $@)
$(WASM_MODULE_OUTPUTS): export CFLAGS += -g0 -O3
$(WASM_MODULE_OUTPUTS): ### Build a specific WASM module
	@echo "# Building WASM module ${MODULE}, requires Rustc for wasm32-wasi."
	${MAYBE_ENVIRONMENT_EXEC} cargo build \
		--manifest-path tests/data/wasm/${MODULE}/Cargo.toml \
		--target wasm32-wasi \
		--release \
		--package ${MODULE}

.PHONY: test-wasm
test-wasm: export TEST_THREADS=1
test-wasm: export TEST_LOG=vector=trace
test-wasm: $(WASM_MODULE_OUTPUTS)  ### Run engine tests
	${MAYBE_ENVIRONMENT_EXEC} cargo test wasm --no-fail-fast --no-default-features --features "transforms-field_filter transforms-wasm transforms-lua transforms-add_fields"

##@ Benching (Supports `ENVIRONMENT=true`)

.PHONY: bench
bench: ## Run benchmarks in /benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "benches" ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-dnstap
bench-dnstap: ## Run dnstap benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "dnstap-benches" --bench dnstap ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-dnsmsg-parser
bench-dnsmsg-parser: ## Run dnsmsg-parser benches
	${MAYBE_ENVIRONMENT_EXEC} CRITERION_HOME="$(CRITERION_HOME)" cargo bench --manifest-path lib/dnsmsg-parser/Cargo.toml ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-remap-functions
bench-remap-functions: ## Run remap-functions benches
	${MAYBE_ENVIRONMENT_EXEC} CRITERION_HOME="$(CRITERION_HOME)" cargo bench --manifest-path lib/vrl/stdlib/Cargo.toml ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-remap
bench-remap: ## Run remap benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "remap-benches" --bench remap ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-transform
bench-transform: ## Run transform benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "transform-benches" --bench transform ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-wasm
bench-wasm: $(WASM_MODULE_OUTPUTS)  ### Run WASM benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "wasm-benches" --bench wasm ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-languages
bench-languages: $(WASM_MODULE_OUTPUTS)  ### Run language comparison benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "language-benches" --bench languages ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-metrics
bench-metrics: ## Run metrics benches
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "metrics-benches" ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

.PHONY: bench-all
bench-all: ### Run all benches
bench-all: $(WASM_MODULE_OUTPUTS) bench-remap-functions
	${MAYBE_ENVIRONMENT_EXEC} cargo bench --no-default-features --features "benches remap-benches wasm-benches metrics-benches language-benches ${DNSTAP_BENCHES}" ${CARGO_BENCH_FLAGS}
	${MAYBE_ENVIRONMENT_COPY_ARTIFACTS}

##@ Checking

.PHONY: check
check: ## Run prerequisite code checks
	${MAYBE_ENVIRONMENT_EXEC} cargo check --all --no-default-features --features ${DEFAULT_FEATURES}

.PHONY: check-all
check-all: ## Check everything
check-all: check-fmt check-clippy check-style check-docs
check-all: check-version check-examples check-component-features
check-all: check-scripts
check-all: check-helm-lint check-helm-dependencies check-helm-snapshots
check-all: check-kubernetes-yaml

.PHONY: check-component-features
check-component-features: ## Check that all component features are setup properly
	${MAYBE_ENVIRONMENT_EXEC} cargo hack check --each-feature --exclude-features "sources-utils-http sources-utils-tcp-keepalive sources-utils-tcp-socket sources-utils-tls sources-utils-udp sources-utils-unix sinks-utils-udp"

.PHONY: check-clippy
check-clippy: ## Check code with Clippy
	${MAYBE_ENVIRONMENT_EXEC} cargo clippy --workspace --all-targets --features all-integration-tests -- -D warnings

.PHONY: check-docs
check-docs: ## Check that all /docs file are valid
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-docs.sh

.PHONY: check-fmt
check-fmt: ## Check that all files are formatted properly
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-fmt.sh

.PHONY: check-style
check-style: ## Check that all files are styled properly
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-style.sh

.PHONY: check-markdown
check-markdown: ## Check that markdown is styled properly
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-markdown.sh

.PHONY: check-version
check-version: ## Check that Vector's version is correct accounting for recent changes
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-version.rb

.PHONY: check-examples
check-examples: ## Check that the config/examples files are valid
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-examples.sh

.PHONY: check-scripts
check-scripts: ## Check that scipts do not have common mistakes
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-scripts.sh

.PHONY: check-helm-lint
check-helm-lint: ## Check that Helm charts pass helm lint
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-helm-lint.sh

.PHONY: check-helm-dependencies
check-helm-dependencies: ## Check that Helm charts have up-to-date dependencies
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/helm-dependencies.sh validate

.PHONY: check-helm-snapshots
check-helm-snapshots: ## Check that the Helm template snapshots do not diverge from the Helm charts
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/helm-template-snapshot.sh check

.PHONY: check-kubernetes-yaml
check-kubernetes-yaml: ## Check that the generated Kubernetes YAML configs are up to date
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/kubernetes-yaml.sh check

check-events: ## Check that events satisfy patterns set in https://github.com/timberio/vector/blob/master/rfcs/2020-03-17-2064-event-driven-observability.md
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-events.sh

##@ Rustdoc
build-rustdoc: ## Build Vector's Rustdocs
	# This command is mostly intended for use by the build process in timberio/vector-rustdoc
	${MAYBE_ENVIRONMENT_EXEC} cargo doc --no-deps

##@ Packaging

# archives
target/artifacts/vector-${VERSION}-%.tar.gz: export TRIPLE :=$(@:target/artifacts/vector-${VERSION}-%.tar.gz=%)
target/artifacts/vector-${VERSION}-%.tar.gz: override PROFILE =release
target/artifacts/vector-${VERSION}-%.tar.gz: target/%/release/vector.tar.gz
	@echo "Built to ${<}, relocating to ${@}"
	@mkdir -p target/artifacts/
	@cp -v \
		${<} \
		${@}

.PHONY: package
package: build ## Build the Vector archive
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/package-archive.sh

.PHONY: package-x86_64-unknown-linux-gnu-all
package-x86_64-unknown-linux-gnu-all: package-x86_64-unknown-linux-gnu package-deb-x86_64-unknown-linux-gnu package-rpm-x86_64-unknown-linux-gnu # Build all x86_64 GNU packages

.PHONY: package-x86_64-unknown-linux-musl-all
package-x86_64-unknown-linux-musl-all: package-x86_64-unknown-linux-musl # Build all x86_64 MUSL packages

.PHONY: package-aarch64-unknown-linux-musl-all
package-aarch64-unknown-linux-musl-all: package-aarch64-unknown-linux-musl # Build all aarch64 MUSL packages

.PHONY: package-aarch64-unknown-linux-gnu-all
package-aarch64-unknown-linux-gnu-all: package-aarch64-unknown-linux-gnu package-deb-aarch64 package-rpm-aarch64 # Build all aarch64 GNU packages

.PHONY: package-armv7-unknown-linux-gnueabihf-all
package-armv7-unknown-linux-gnueabihf-all: package-armv7-unknown-linux-gnueabihf package-deb-armv7-gnu package-rpm-armv7-gnu  # Build all armv7-unknown-linux-gnueabihf MUSL packages

.PHONY: package-x86_64-unknown-linux-gnu
package-x86_64-unknown-linux-gnu: target/artifacts/vector-${VERSION}-x86_64-unknown-linux-gnu.tar.gz ## Build an archive suitable for the `x86_64-unknown-linux-gnu` triple.
	@echo "Output to ${<}."

.PHONY: package-x86_64-unknown-linux-musl
package-x86_64-unknown-linux-musl: target/artifacts/vector-${VERSION}-x86_64-unknown-linux-musl.tar.gz ## Build an archive suitable for the `x86_64-unknown-linux-musl` triple.
	@echo "Output to ${<}."

.PHONY: package-aarch64-unknown-linux-musl
package-aarch64-unknown-linux-musl: target/artifacts/vector-${VERSION}-aarch64-unknown-linux-musl.tar.gz ## Build an archive suitable for the `aarch64-unknown-linux-musl` triple.
	@echo "Output to ${<}."

.PHONY: package-aarch64-unknown-linux-gnu
package-aarch64-unknown-linux-gnu: target/artifacts/vector-${VERSION}-aarch64-unknown-linux-gnu.tar.gz ## Build an archive suitable for the `aarch64-unknown-linux-gnu` triple.
	@echo "Output to ${<}."

.PHONY: package-armv7-unknown-linux-gnueabihf
package-armv7-unknown-linux-gnueabihf: target/artifacts/vector-${VERSION}-armv7-unknown-linux-gnueabihf.tar.gz ## Build an archive suitable for the `armv7-unknown-linux-gnueabihf` triple.
	@echo "Output to ${<}."

.PHONY: package-armv7-unknown-linux-musleabihf
package-armv7-unknown-linux-musleabihf: target/artifacts/vector-${VERSION}-armv7-unknown-linux-musleabihf.tar.gz ## Build an archive suitable for the `armv7-unknown-linux-musleabihf triple.
	@echo "Output to ${<}."

# debs

.PHONY: package-deb-x86_64-unknown-linux-gnu
package-deb-x86_64-unknown-linux-gnu: package-x86_64-unknown-linux-gnu ## Build the x86_64 GNU deb package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=x86_64-unknown-linux-gnu timberio/ci_image ./scripts/package-deb.sh

.PHONY: package-deb-x86_64-unknown-linux-musl
package-deb-x86_64-unknown-linux-musl: package-x86_64-unknown-linux-musl ## Build the x86_64 GNU deb package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=x86_64-unknown-linux-musl timberio/ci_image ./scripts/package-deb.sh

.PHONY: package-deb-aarch64
package-deb-aarch64: package-aarch64-unknown-linux-gnu ## Build the aarch64 deb package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=aarch64-unknown-linux-gnu timberio/ci_image ./scripts/package-deb.sh

.PHONY: package-deb-armv7-gnu
package-deb-armv7-gnu: package-armv7-unknown-linux-gnueabihf ## Build the armv7-unknown-linux-gnueabihf deb package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=armv7-unknown-linux-gnueabihf timberio/ci_image ./scripts/package-deb.sh

# rpms

.PHONY: package-rpm-x86_64-unknown-linux-gnu
package-rpm-x86_64-unknown-linux-gnu: package-x86_64-unknown-linux-gnu ## Build the x86_64 rpm package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=x86_64-unknown-linux-gnu timberio/ci_image ./scripts/package-rpm.sh

.PHONY: package-rpm-x86_64-unknown-linux-musl
package-rpm-x86_64-unknown-linux-musl: package-x86_64-unknown-linux-musl ## Build the x86_64 musl rpm package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=x86_64-unknown-linux-musl timberio/ci_image ./scripts/package-rpm.sh

.PHONY: package-rpm-aarch64
package-rpm-aarch64: package-aarch64-unknown-linux-gnu ## Build the aarch64 rpm package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=aarch64-unknown-linux-gnu timberio/ci_image ./scripts/package-rpm.sh

.PHONY: package-rpm-armv7-gnu
package-rpm-armv7-gnu: package-armv7-unknown-linux-gnueabihf ## Build the armv7-unknown-linux-gnueabihf rpm package
	$(CONTAINER_TOOL) run -v  $(PWD):/git/timberio/vector/ -e TARGET=armv7-unknown-linux-gnueabihf timberio/ci_image ./scripts/package-rpm.sh

##@ Releasing

.PHONY: release
release: release-prepare generate release-commit ## Release a new Vector version

.PHONY: release-commit
release-commit: ## Commits release changes
	@scripts/release-commit.rb

.PHONY: release-docker
release-docker: ## Release to Docker Hub
	@scripts/build-docker.sh

.PHONY: release-github
release-github: ## Release to Github
	@scripts/release-github.sh

.PHONY: release-homebrew
release-homebrew: ## Release to timberio Homebrew tap
	@scripts/release-homebrew.sh

.PHONY: release-prepare
release-prepare: ## Prepares the release with metadata and highlights
	@scripts/release-prepare.rb

.PHONY: release-push
release-push: ## Push new Vector version
	@scripts/release-push.sh

.PHONY: release-rollback
release-rollback: ## Rollback pending release changes
	@scripts/release-rollback.rb

.PHONY: release-s3
release-s3: ## Release artifacts to S3
	@scripts/release-s3.sh

.PHONY: release-helm
release-helm: ## Package and release Helm Chart
	@scripts/release-helm.sh

.PHONY: sync-install
sync-install: ## Sync the install.sh script for access via sh.vector.dev
	@aws s3 cp distribution/install.sh s3://sh.vector.dev --sse --acl public-read

##@ Vector Remap Language

.PHONY: test-vrl
test-vrl: ## Run the VRL test suite
	@scripts/test-vrl.sh

##@ Utility

.PHONY: build-ci-docker-images
build-ci-docker-images: ## Rebuilds all Docker images used in CI
	@scripts/build-ci-docker-images.sh

.PHONY: clean
clean: environment-clean ## Clean everything
	cargo clean

.PHONY: fmt
fmt: ## Format code
	${MAYBE_ENVIRONMENT_EXEC} cargo fmt
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/check-style.sh --fix

.PHONY: signoff
signoff: ## Signsoff all previous commits since branch creation
	scripts/signoff.sh

.PHONY: slim-builds
slim-builds: ## Updates the Cargo config to product disk optimized builds (for CI, not for users)
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/slim-builds.sh

ifeq (${CI}, true)
.PHONY: ci-sweep
ci-sweep: ## Sweep up the CI to try to get more disk space.
	@echo "Preparing the CI for build by sweeping up disk space a bit..."
	df -h
	sudo apt-get --purge autoremove --yes
	sudo apt-get clean
	sudo rm -rf "/opt/*" "/usr/local/*"
	sudo rm -rf "/usr/local/share/boost" && sudo rm -rf "${AGENT_TOOLSDIRECTORY}"
	docker system prune --force
	df -h
endif

.PHONY: version
version: ## Get the current Vector version
	@scripts/version.sh

.PHONY: git-hooks
git-hooks: ## Add Vector-local git hooks for commit sign-off
	@scripts/install-git-hooks.sh

.PHONY: update-helm-dependencies
update-helm-dependencies: ## Recursively update the dependencies of the Helm charts in the proper order
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/helm-dependencies.sh update

.PHONY: update-helm-snapshots
update-helm-snapshots: ## Update the Helm template snapshots from the Helm charts
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/helm-template-snapshot.sh update

.PHONY: update-kubernetes-yaml
update-kubernetes-yaml: ## Regenerate the Kubernetes YAML configs
	${MAYBE_ENVIRONMENT_EXEC} ./scripts/kubernetes-yaml.sh update

.PHONY: cargo-install-%
cargo-install-%: override TOOL = $(@:cargo-install-%=%)
cargo-install-%:
	$(if $(findstring true,$(AUTOINSTALL)),cargo install ${TOOL} --quiet; cargo clean,)

.PHONY: ensure-has-wasm-toolchain ### Configures a wasm toolchain for test artifact building, if required
ensure-has-wasm-toolchain: target/wasm32-wasi/.obtained
target/wasm32-wasi/.obtained:
	@echo "# You should also install WABT for WASM module development!"
	@echo "# You can use your package manager or check https://github.com/WebAssembly/wabt"
	${MAYBE_ENVIRONMENT_EXEC} rustup target add wasm32-wasi
	@mkdir -p target/wasm32-wasi
	@touch target/wasm32-wasi/.obtained
