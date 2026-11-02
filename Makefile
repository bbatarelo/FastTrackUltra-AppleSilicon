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
	@echo 'make test-hal-independent  Same HAL boundary/hotplug tests on the independent session (no hardware)'
	@echo 'make test-install  Test deployment in temporary directories (no system changes)'
	@echo 'make test-prototype-4ms  Build/test the isolated 4 ms prototype (no hardware)'
	@echo 'make test-prototype-4ms-ten-slot  Build/test the isolated ten-slot prototype (no hardware)'
	@echo 'make test-prototype-4ms-deadline  Test bounded deadline scheduling (no hardware)'
	@echo 'make test-prototype-4ms-ready  Build/test bounded PCM preparation (no hardware)'
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
$(BUNDLE)/Contents/MacOS/FastTrackUltra: $(BUILD)/baseline-transport.json $(BUILD)/build_identity.h driver/plugin.c driver/Info.plist LICENSE scripts/sign.sh $(NATIVE) shared/ftu_live.h shared/ftu_native.h $(RUST_LIB)
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
$(BUILD)/test-hotplug-discovery: tests/hotplug-discovery.c $(NATIVE) shared/ftu_live.h shared/ftu_native.h $(RUST_LIB) | $(BUILD)
	clang $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $@ tests/hotplug-discovery.c $(RUST_LIB) $(FRAMEWORKS)
test-hal: $(BUILD)/test-hal-boundary $(BUILD)/test-hotplug-discovery
	$(BUILD)/test-hal-boundary
	$(BUILD)/test-hotplug-discovery
# Same boundary and hotplug regressions against the independent-4ms session.
# Separate Cargo artifacts; never the candidate bundle's build directory.
HAL_INDEPENDENT := $(BUILD)/hal-independent
.PHONY: test-hal-independent
test-hal-independent: | $(BUILD)
	CARGO_TARGET_DIR=$(HAL_INDEPENDENT)/target MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --release --locked --offline -p ftu-stream --no-default-features --features independent-4ms
	clang $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -mmacosx-version-min=14.0 -DFTU_INDEPENDENT -o $(HAL_INDEPENDENT)/test-hal-boundary tests/hal-boundary.c tools/ftu-probe/completion_route.c $(HAL_INDEPENDENT)/target/release/libftu_stream.a $(FRAMEWORKS)
	$(HAL_INDEPENDENT)/test-hal-boundary

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
# Extended instrument and read-only observer are opt-in developer tools.
$(BUILD)/hal-soak: tools/hal-loopback/main.c tools/hal-loopback/permission.m tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -DFTU_LOOPBACK_MAX_SECONDS=600 -o $@ tools/hal-loopback/main.c tools/hal-loopback/permission.m -framework CoreAudio -framework CoreFoundation -framework AVFoundation -framework Foundation
$(BUILD)/hal-observe: tools/hal-check/observe.c tools/hal-check/main.c | $(BUILD)
	clang $(CFLAGS) -o $@ $< -framework CoreAudio -framework CoreFoundation
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

# Deliberately separate Cargo artifacts: ordinary make install stays baseline.
.PHONY: prototype-4ms test-prototype-4ms
prototype-4ms:
	python3 scripts/build_prototype.py
test-prototype-4ms:
	python3 scripts/build_prototype.py --test

$(BUILD)/transport-profile: tests/transport-profile.c shared/ftu_native.h $(RUST_LIB) | $(BUILD)
	clang $(CFLAGS) -o $@ tests/transport-profile.c $(RUST_LIB) $(FRAMEWORKS)
$(BUILD)/baseline-transport.json: $(BUILD)/transport-profile
	$(BUILD)/transport-profile 8 4 > $@.tmp
	mv $@.tmp $@

.PHONY: prototype-4ms-ten-slot test-prototype-4ms-ten-slot
prototype-4ms-ten-slot:
	python3 scripts/build_prototype.py --slots 10
test-prototype-4ms-ten-slot:
	python3 scripts/build_prototype.py --slots 10 --test

.PHONY: prototype-4ms-ready test-prototype-4ms-ready
prototype-4ms-ready:
	python3 scripts/build_prototype.py --slots 10 --pcm-ready
test-prototype-4ms-ready:
	python3 scripts/build_prototype.py --slots 10 --pcm-ready --test

.PHONY: prototype-4ms-deadline test-prototype-4ms-deadline
prototype-4ms-deadline:
	python3 scripts/build_prototype.py --slots 10 --pcm-ready --deadline
test-prototype-4ms-deadline:
	python3 scripts/build_prototype.py --slots 10 --pcm-ready --deadline --test

# Explicit developer hardware candidate; ordinary make install stays baseline.
.PHONY: independent-driver test-independent
independent-driver:
	python3 scripts/build_independent.py
test-independent:
	python3 tools/ftu-probe/test_live_completion.py

# A recovered stream is still a failed uninterrupted-audio qualification run.
$(BUILD)/test-session-guard: tests/session-guard.c tools/hal-check/session_guard.h | $(BUILD)
	clang $(CFLAGS) -O1 -fsanitize=address,undefined -o $@ $<
.PHONY: test-session-guard
test-session-guard: $(BUILD)/test-session-guard
	$(BUILD)/test-session-guard
test-loopback: test-session-guard
$(BUILD)/hal-check $(BUILD)/hal-scenarios $(BUILD)/hal-loopback $(BUILD)/hal-soak $(BUILD)/hal-observe: tools/hal-check/session_guard.h
