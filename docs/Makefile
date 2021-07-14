CUE = ./scripts/cue.sh

clean:
	rm -rf public resources

setup:
	yarn

# Build JSON from CUE sources
cue-build:
	${CUE} build

# Other CUE commands
cue-check:
	${CUE} check

cue-fmt:
	${CUE} fmt

cue-list:
	${CUE} list

cue-vet:
	${CUE} vet

config-examples:
	yarn run config-examples

structured-data: cue-build config-examples

serve: setup structured-data
	hugo server \
	--buildDrafts \
	--buildFuture \
	--environment "development"

# Live site
production-build:
	hugo \
	--environment "production" \
	--minify

ci-production-build: setup structured-data production-build run-link-checker algolia

# Preview site
preview-build:
	hugo \
	--baseURL $(DEPLOY_PRIME_URL) \
	--buildFuture \
	--environment "preview" \
	--minify

ci-preview-build: setup structured-data preview-build algolia

run-link-checker:
	htmltest

run-external-link-checker:
	htmltest --conf ./.htmltest.external.yml

# Check links without rebuilding the CUE sources. Optimal for fixing links locally.
check-links: production-build run-link-checker

# Search
algolia:
	yarn run algolia-config
	yarn run algolia-index
	yarn run algolia-upload

# Useful for locally debugging issues that arise only on the deployed production site
run-production-site-locally:
	make setup cue-build production-build
	python -m http.server 8000 --directory ./public --bind 127.0.0.1

# Full local builds without Algolia updates (for debugging, link checking, etc.)
local-production-build: clean setup structured-data production-build run-link-checker

local-preview-build: clean setup structured-data preview-build run-link-checker
