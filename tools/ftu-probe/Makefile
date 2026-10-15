CC = cc
CFLAGS = -std=c11 -Wall -Wextra -Werror -O2 -g
# Newer SDKs may annotate parts of the legacy IOUSBLib API as deprecated;
# keep -Werror for everything else.
CFLAGS += -Wno-deprecated-declarations
LDLIBS = -framework IOKit -framework CoreFoundation

BUILD = build
CAPTURE = capture.c duplex.c
RUST_LIB = ../../target/release/libftu_stream.a
RUST_SOURCES = $(shell find ../../rust -type f \( -name '*.rs' -o -name Cargo.toml \)) ../../Cargo.toml ../../Cargo.lock
CAPTURE_HEADERS = capture.h capture_model.h capture_profile.h feedback_profile.h pcm.h duplex.h ../../shared/ftu_native.h

.PHONY: all test sanitize clean rust-test
FIXTURE = ../../tests/fixtures/descriptors/ultra-config-0.bin

all: $(BUILD)/ftu-probe

$(BUILD)/ftu-probe: main.c controls_json.c controls.h $(CAPTURE) $(CAPTURE_HEADERS) $(RUST_LIB) | $(BUILD)
	$(CC) $(CFLAGS) -o $@ main.c controls_json.c $(CAPTURE) $(RUST_LIB) $(LDLIBS) -framework Security -liconv

$(RUST_LIB): $(RUST_SOURCES)
	cargo build --manifest-path ../../Cargo.toml --release --offline -p ftu-stream

# Original C implementations remain offline reference tests.
$(BUILD)/test_controls: test_controls.c controls.c controls.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_controls.c controls.c

$(BUILD)/test_capture_model: test_capture_model.c capture_model.c capture_model.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_capture_model.c capture_model.c

$(BUILD)/test_pcm: test_pcm.c pcm.c pcm.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_pcm.c pcm.c

$(BUILD)/test_capture_profile: test_capture_profile.c capture_profile.c capture_profile.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_capture_profile.c capture_profile.c

$(BUILD)/test_feedback_profile: test_feedback_profile.c feedback_profile.c feedback_profile.h capture_model.h | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_feedback_profile.c feedback_profile.c

$(BUILD):
	mkdir -p $(BUILD)

test: rust-test $(BUILD)/test_pool_native $(BUILD)/test_feedback_profile $(BUILD)/ftu-probe $(BUILD)/test_controls $(BUILD)/test_capture_model $(BUILD)/test_pcm $(BUILD)/test_capture_profile
	$(BUILD)/test_pool_native
	$(BUILD)/test_controls
	$(BUILD)/test_capture_model
	$(BUILD)/test_pcm
	$(BUILD)/test_capture_profile $(FIXTURE)
	$(BUILD)/test_feedback_profile $(FIXTURE)
	FTU_PROBE=$(BUILD)/ftu-probe python3 -B -m unittest -v test_decode test_control_plan test_hardware_fixture test_capture_cli test_capture_evidence test_feedback_evidence test_duplex_evidence test_stream_evidence

sanitize: | $(BUILD)
	$(CC) $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $(BUILD)/test_pcm_sanitize test_pcm.c pcm.c
	$(CC) $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $(BUILD)/test_model_sanitize test_capture_model.c capture_model.c
	$(CC) $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $(BUILD)/test_profile_sanitize test_capture_profile.c capture_profile.c
	$(CC) $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $(BUILD)/test_feedback_sanitize test_feedback_profile.c feedback_profile.c
	$(BUILD)/test_feedback_sanitize $(FIXTURE)
	$(BUILD)/test_pcm_sanitize
	$(BUILD)/test_model_sanitize
	$(BUILD)/test_profile_sanitize $(FIXTURE)

clean:
	rm -rf $(BUILD)

# The same independent C acceptance tests linked against the Rust ABI.
rust-test: $(RUST_LIB) | $(BUILD)
	cargo test --manifest-path ../../Cargo.toml --workspace --release --offline
	$(CC) $(CFLAGS) -o $(BUILD)/test_controls_rust test_controls.c controls_json.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(CC) $(CFLAGS) -o $(BUILD)/test_model_rust test_capture_model.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(CC) $(CFLAGS) -o $(BUILD)/test_pcm_rust test_pcm.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(CC) $(CFLAGS) -o $(BUILD)/test_profile_rust test_capture_profile.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(CC) $(CFLAGS) -o $(BUILD)/test_feedback_rust test_feedback_profile.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(BUILD)/test_controls_rust
	$(BUILD)/test_model_rust
	$(BUILD)/test_pcm_rust
	$(BUILD)/test_profile_rust $(FIXTURE)
	$(BUILD)/test_feedback_rust $(FIXTURE)

$(BUILD)/test_pool_native: test_pool_native.c duplex.c duplex.h ../../shared/ftu_native.h $(RUST_LIB) | $(BUILD)
	$(CC) $(CFLAGS) -o $@ test_pool_native.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv

.PHONY: sanitize-native
sanitize-native: $(RUST_LIB) | $(BUILD)
	$(CC) $(CFLAGS) -O1 -fsanitize=address,undefined -fno-omit-frame-pointer -o $(BUILD)/test_pool_native_sanitize test_pool_native.c $(RUST_LIB) $(LDLIBS) -framework Security -liconv
	$(BUILD)/test_pool_native_sanitize
