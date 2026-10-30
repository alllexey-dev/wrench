.PHONY: build build-dist build-examples build-fmt \
        test test-rs test-examples test-server test-accept test-perf \
        format format-rs format-asm format-py format-md format-html format-yaml \
        format-check format-check-rs format-check-asm format-check-py format-check-html format-check-md \
        lint lint-rs lint-py lint-html typecheck-py \
        lint-fix lint-fix-rs lint-fix-py \
        generate generate-variants \
        run-server \
        docs-rustdoc \
        docker-build docker-push-edge \
        fix clean

COMMIT = $(shell git rev-parse --short HEAD)

IMAGE_NAME = ryukzak/wrench

EDGE_IMAGE = $(IMAGE_NAME):edge
COMMIT_IMAGE = $(IMAGE_NAME):$(COMMIT)

WRENCH = target/release/wrench
WRENCH_FMT = target/release/wrench-fmt

export VERSION_SUFFIX ?= DEV

all: format lint-fix test

# Build

build:
	cargo build --release --workspace

build-fmt:
	cargo build --release --bin wrench-fmt

build-examples: build
	python script/build_examples.py --wrench $(WRENCH) --fail-fast

# Docs

docs-rustdoc:
	cargo doc --no-deps --workspace

# Run

run-server: build generate-variants
	WRENCH_EXEC=$(WRENCH) target/release/wrench-serv

# Test

test: test-rs test-examples test-server

test-rs:
	cargo test --workspace

test-examples: build
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/factorial.s    -c example/risc-iv-32/factorial-5.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/hello.s        -c example/risc-iv-32/hello.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/get-put-char.s -c example/risc-iv-32/get-put-char-87.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/get-put-char.s -c example/risc-iv-32/get-put-char-ABCD.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/not.s          -c example/risc-iv-32/not-true.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/huge-report.s  -c example/risc-iv-32/huge-report.yaml
	$(WRENCH) --isa risc-iv-32 example/risc-iv-32/random-memory-init.s -c example/risc-iv-32/random-memory-init.yaml

	$(WRENCH) --isa f32a       example/f32a/not.s                -c example/f32a/not-true.yaml
	$(WRENCH) --isa f32a       example/f32a/hello.s              -c example/f32a/hello.yaml
	# $(WRENCH) --isa f32a       example/f32a/get-put-char.s       -c example/f32a/get-put-char-87.yaml
	# $(WRENCH) --isa f32a       example/f32a/get-put-char.s       -c example/f32a/get-put-char-ABCD.yaml
	$(WRENCH) --isa f32a       example/f32a/factorial.s          -c example/f32a/factorial.yaml

	$(WRENCH) --isa acc32      example/acc32/not.s               -c example/acc32/not-true.yaml
	$(WRENCH) --isa acc32      example/acc32/hello.s             -c example/acc32/hello.yaml
	$(WRENCH) --isa acc32      example/acc32/get-put-char.s      -c example/acc32/get-put-char-87.yaml
	$(WRENCH) --isa acc32      example/acc32/get-put-char.s      -c example/acc32/get-put-char-ABCD.yaml
	$(WRENCH) --isa acc32      example/acc32/factorial.s         -c example/acc32/factorial-5.yaml

	$(WRENCH) --isa m68k       example/m68k/not.s                -c example/m68k/not-true.yaml
	$(WRENCH) --isa m68k       example/m68k/get-put-char.s       -c example/m68k/get-put-char-87.yaml
	$(WRENCH) --isa m68k       example/m68k/hello.s              -c example/m68k/hello.yaml
	$(WRENCH) --isa m68k       example/m68k/hello-byte.s         -c example/m68k/hello-byte.yaml
	$(WRENCH) --isa m68k       example/m68k/factorial.s          -c example/m68k/factorial-5.yaml
	$(WRENCH) --isa m68k       example/m68k/factorial-recursive.s -c example/m68k/factorial-recursive.yaml
	$(WRENCH) --isa m68k       example/m68k/factorial-recursive-2.s -c example/m68k/factorial-recursive-2.yaml

	$(WRENCH) --isa vliw-iv    example/vliw-iv/hello.s           -c example/vliw-iv/hello.yaml
	$(WRENCH) --isa vliw-iv    example/vliw-iv/factorial.s       -c example/vliw-iv/factorial-5.yaml
	$(WRENCH) --isa vliw-iv    example/vliw-iv/test-parallel.s   -c example/vliw-iv/test-parallel.yaml

	$(WRENCH) --isa wasm32     example/wasm32/hello.s            -c example/wasm32/hello.yaml
	$(WRENCH) --isa wasm32     example/wasm32/factorial.s        -c example/wasm32/factorial-5.yaml
	$(WRENCH) --isa wasm32     example/wasm32/factorial-recursive.s -c example/wasm32/factorial-recursive-5.yaml
	$(WRENCH) --isa wasm32     example/wasm32/get-put-char.s     -c example/wasm32/get-put-char-87.yaml
	$(WRENCH) --isa wasm32     example/wasm32/get-put-char.s     -c example/wasm32/get-put-char-ABCD.yaml
	$(WRENCH) --isa wasm32     example/wasm32/not.s              -c example/wasm32/not-true.yaml
	$(WRENCH) --isa wasm32     example/wasm32/frames.s           -c example/wasm32/frames.yaml

test-server: build generate-variants
	WRENCH_EXEC=$(WRENCH) target/release/wrench-serv &
	hurl --retry 3 --no-output test/wrench-serv.hurl
	pkill -f wrench-serv

test-perf:
	cargo build --release --bin wrench
	time $(WRENCH) --isa f32a test/performance/program.s -c test/performance/conf.yaml

test-accept: generate-variants
	WRENCH_ACCEPT=1 cargo test --workspace

# Format – apply

format: format-rs format-asm format-py format-md format-html format-yaml

format-rs:
	cargo fmt --all

format-asm: build-fmt
	$(WRENCH_FMT) --inplace --isa risc-iv-32 -v example/risc-iv-32/*.s test/golden/risc-iv-32/*.s
	$(WRENCH_FMT) --inplace --isa f32a       -v example/f32a/*.s       test/golden/f32a/*.s
	$(WRENCH_FMT) --inplace --isa acc32      -v example/acc32/*.s      test/golden/acc32/*.s
	$(WRENCH_FMT) --inplace --isa m68k       -v example/m68k/*.s       test/golden/m68k/*.s
	$(WRENCH_FMT) --inplace --isa vliw-iv    -v example/vliw-iv/*.s    test/golden/vliw-iv/*.s
	$(WRENCH_FMT) --inplace --isa wasm32     -v example/wasm32/*.s     test/golden/wasm32/*.s

format-py:
	ruff format script

format-md:
	markdownlint . .rules -c .markdownlint.yaml --fix

format-html:
	npx @biomejs/biome format --write static/

format-yaml:
	yamlfmt package.yaml example test .github/workflows

# Format – check

format-check: format-check-rs format-check-asm format-check-py format-check-html format-check-md

format-check-rs:
	cargo fmt --all --check

format-check-asm: build-fmt
	$(WRENCH_FMT) --check --isa risc-iv-32 -v example/risc-iv-32/*.s test/golden/risc-iv-32/*.s
	$(WRENCH_FMT) --check --isa f32a       -v example/f32a/*.s       test/golden/f32a/*.s
	$(WRENCH_FMT) --check --isa acc32      -v example/acc32/*.s      test/golden/acc32/*.s
	$(WRENCH_FMT) --check --isa m68k       -v example/m68k/*.s       test/golden/m68k/*.s
	$(WRENCH_FMT) --check --isa vliw-iv    -v example/vliw-iv/*.s    test/golden/vliw-iv/*.s
	$(WRENCH_FMT) --check --isa wasm32     -v example/wasm32/*.s     test/golden/wasm32/*.s

format-check-py:
	ruff format --check script

format-check-html:
	npx @biomejs/biome format static/

format-check-md:
	markdownlint . .rules -c .markdownlint.yaml

# Lint – check

lint: lint-rs lint-py lint-html typecheck-py

lint-rs:
	cargo clippy --workspace --all-targets -- -D warnings

lint-py:
	ruff check script

typecheck-py:
	pyright

lint-html:
	npx @biomejs/biome lint static/

# Lint – fix

lint-fix: lint-fix-rs lint-fix-py

lint-fix-rs:
	cargo clippy --workspace --all-targets --fix --allow-dirty

lint-fix-py:
	ruff check script --fix

# Generate

generate: generate-variants

generate-variants:
	mkdir -p variants
	script/variants.py

# Docker

docker-build:
	docker build --build-arg VERSION_SUFFIX=DEV -t $(IMAGE_NAME) .

docker-push-edge:
	docker buildx build --build-arg VERSION_SUFFIX=EDGE --platform linux/amd64,linux/arm64 --push \
		-t $(EDGE_IMAGE) -t $(COMMIT_IMAGE) .

docker-run-server: docker-build
	docker run -it --rm $(IMAGE_NAME) wrench-serv

# Meta

fix: lint-fix format generate test-accept test-examples test-server

fix-variant: format-py lint-fix-py generate-variants

clean:
	cargo clean
	fd .result | xargs rm -v
	rm -v -R -f test/golden/generated/*
	rm -v -R -f variants/*
	rm -v -R -f variants.md
