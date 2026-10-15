SHELL := /bin/sh
.DEFAULT_GOAL := all
BUILD := build
RUST_LIB := target/release/libftu_stream.a
CFLAGS := -std=c11 -O2 -g -Wall -Wextra -Werror -Wno-deprecated-declarations
FRAMEWORKS := -framework CoreAudio -framework CoreFoundation -framework IOKit -framework Security -liconv
RUST_SOURCES := $(shell find rust -type f \( -name '*.rs' -o -name Cargo.toml \)) Cargo.toml Cargo.lock
NATIVE := tools/ftu-probe/duplex.c
.PHONY: all test driver tools doctor help install uninstall check version package test-install
all: driver tools
tools: $(BUILD)/live-check $(BUILD)/hal-check $(BUILD)/hal-host-check $(BUILD)/hal-scenarios $(BUILD)/hal-loopback
doctor:
	@/bin/sh scripts/check_build_environment.sh
help:
	@echo 'make             Build the driver and diagnostic tools'
	@echo 'make install     Build, back up the previous driver, install and restart audio'
	@echo 'make uninstall   Remove this driver, keep a backup and restart audio'
	@echo 'make check       Inspect the loaded driver without starting audio'
	@echo 'make version     Show the version and content identity this source builds'
	@echo 'make package     Build local macOS install and uninstall packages'
	@echo 'make test        Run the protocol/transport regression tests'
	@echo 'make test-hal    Run HAL boundary sanitizer tests (no hardware)'
	@echo 'make test-install  Test deployment in temporary directories (no system changes)'
	@echo 'make doctor      Check source-build prerequisites'
$(RUST_LIB): $(RUST_SOURCES)
	cargo build --release --locked --offline -p ftu-stream
$(BUILD):
	mkdir -p $(BUILD)
$(BUILD)/live-check: tools/live-check/main.c $(NATIVE) shared/ftu_live.h shared/ftu_native.h $(RUST_LIB) | $(BUILD)
	clang $(CFLAGS) -o $@ tools/live-check/main.c $(NATIVE) $(RUST_LIB) $(FRAMEWORKS)
test:
	$(MAKE) -C tools/ftu-probe test

BUNDLE := $(BUILD)/FastTrackUltra.driver
driver: doctor $(BUNDLE)/Contents/MacOS/FastTrackUltra
$(BUNDLE)/Contents/MacOS/FastTrackUltra: $(BUILD)/build_identity.h driver/plugin.c driver/Info.plist LICENSE scripts/sign.sh $(NATIVE) shared/ftu_live.h shared/ftu_native.h $(RUST_LIB)
	rm -rf $(BUNDLE)
	mkdir -p $(BUNDLE)/Contents/MacOS $(BUNDLE)/Contents/Resources
	cp $(BUILD)/Info.plist $(BUNDLE)/Contents/Info.plist
	cp LICENSE $(BUNDLE)/Contents/Resources/LICENSE
	clang $(CFLAGS) -arch arm64 -include $(BUILD)/build_identity.h -bundle -mmacosx-version-min=14.0 -o $@ driver/plugin.c $(NATIVE) $(RUST_LIB) $(FRAMEWORKS)
	@if [ -d $@.dSYM ]; then rm -rf $(BUILD)/FastTrackUltra.dSYM; mv $@.dSYM $(BUILD)/FastTrackUltra.dSYM; fi
	/bin/sh scripts/sign.sh $(BUNDLE)
$(BUILD)/test-hal-boundary: tests/hal-boundary.c driver/plugin.c shared/ftu_live.h $(RUST_LIB) | $(BUILD)
	clang $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $@ tests/hal-boundary.c $(RUST_LIB) $(FRAMEWORKS)
.PHONY: test-hal
test-hal: $(BUILD)/test-hal-boundary
	$(BUILD)/test-hal-boundary

.PHONY: FORCE
FORCE:
$(BUILD)/build_identity.h: FORCE | $(BUILD)
	python3 scripts/build_identity.py
$(BUILD)/hal-host-check: tools/hal-host-check/main.c | $(BUILD)
	clang $(CFLAGS) -o $@ $< $(FRAMEWORKS)

$(BUILD)/hal-check: tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -o $@ $< -framework CoreAudio -framework CoreFoundation
$(BUILD)/hal-scenarios: tools/hal-check/scenarios.c tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -o $@ $< -framework CoreAudio -framework CoreFoundation
$(BUILD)/hal-loopback: tools/hal-loopback/main.c tools/hal-loopback/permission.m tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -o $@ tools/hal-loopback/main.c tools/hal-loopback/permission.m -framework CoreAudio -framework CoreFoundation -framework AVFoundation -framework Foundation
$(BUILD)/test-loopback-callback: tools/hal-loopback/test_callback.c tools/hal-loopback/main.c tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -O1 -Wno-unused-function -fsanitize=address,undefined -o $@ $< -framework CoreAudio -framework CoreFoundation
.PHONY: test-loopback
test-loopback: $(BUILD)/test-loopback-callback
	$(BUILD)/test-loopback-callback
	python3 -m unittest discover -s tools/hal-loopback -p 'test_*.py' -v
.PHONY: check-driver
check-driver: driver
	python3 scripts/manage_hal.py check

install: check-driver
	@echo 'Installing Fast Track Ultra (administrator password required; audio will restart).'
	sudo /bin/sh scripts/deploy_hal.sh install "$(CURDIR)/$(BUNDLE)" --restart
	@echo 'Installed. Select Fast Track Ultra in System Settings > Sound. Run make check to inspect it.'

uninstall:
	sudo /bin/sh scripts/deploy_hal.sh remove --restart

check: $(BUILD)/hal-check
	@$(BUILD)/hal-check

version: $(BUILD)/build_identity.h
	@python3 -c 'import json; m=json.load(open("build/build-identity.json")); print(m["version"] + " (dev-" + m["identity"][:16] + ")")'

package: check-driver
	python3 scripts/package.py

test-install:
	python3 -m unittest discover -s tests -p 'test_installation.py' -v

.PHONY: test-app test-fixtures
test-app:
	python3 tests/test_hal_app_audit.py

test-fixtures:
	python3 tests/test_fixtures.py
