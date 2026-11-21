IMAGE ?= shopping-sessions:latest
CPUS ?= 8
MEM ?= 24g
PLATFORM ?= linux/amd64
DEV ?= dev
ANSWER ?= dev-answer
export IMAGE CPUS MEM PLATFORM

.PHONY: build fixtures adversarial dev run-dev check-dev native
build:
	docker buildx build --platform $(PLATFORM) --load -t $(IMAGE) .
fixtures: build
	python3 scripts/run_fixtures.py fixtures -- sh scripts/docker_run.sh
adversarial: build
	python3 tests/generate_cases.py --output scratch/adversarial
	python3 scripts/run_fixtures.py scratch/adversarial -- sh scripts/docker_run.sh
dev:
	generate --seed 1 --events 1000000 --clicks 250000 --output $(DEV)
run-dev: build
	sh scripts/docker_run.sh $(DEV) out
check-dev:
	python3 scripts/compare.py out $(ANSWER)
native:
	cargo build --release --locked
