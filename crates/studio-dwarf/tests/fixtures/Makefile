# Makefile for building ARM ELF test fixtures

CC = arm-none-eabi-gcc
CXX = arm-none-eabi-g++
CFLAGS = -mcpu=cortex-m4 -mthumb -g3 -O0 -Wall -mfloat-abi=soft
CXXFLAGS = $(CFLAGS) -std=c++17 -fno-exceptions -fno-rtti
LDFLAGS = -T link.ld -nostartfiles -lgcc
CXXLDFLAGS = -T link.ld -nostartfiles -nostdlib -lgcc

C_TARGETS = test_arm.elf test_struct.elf test_pointer.elf test_complex_c.elf
CXX_TARGETS = test_cpp.elf
RUST_TARGETS = test_rust.elf

TARGETS = $(C_TARGETS) $(CXX_TARGETS)

all: $(TARGETS) rust

# C targets
test_arm.elf: test_source.c link.ld
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<

test_struct.elf: test_struct_source.c link.ld
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<

test_pointer.elf: test_pointer_source.c link.ld
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<

test_complex_c.elf: test_complex_c_source.c link.ld
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<

# C++ targets
test_cpp.elf: test_cpp_simple.cpp link.ld
	$(CXX) $(CXXFLAGS) $(LDFLAGS) -o $@ $<

# Rust target (requires rust toolchain)
rust: $(RUST_TARGETS)

test_rust.elf:
	@if command -v rustup >/dev/null 2>&1; then \
		cd rust_embedded && \
		rustup target add thumbv7em-none-eabi 2>/dev/null || true && \
		cargo build --release && \
		cp target/thumbv7em-none-eabi/release/test_rust_embedded ../test_rust.elf; \
	else \
		echo "Warning: Rust toolchain not found, skipping test_rust.elf"; \
	fi

clean:
	rm -f $(C_TARGETS) $(CXX_TARGETS) $(RUST_TARGETS)
	@if [ -d rust_embedded ]; then \
		cd rust_embedded && cargo clean 2>/dev/null || true; \
	fi

.PHONY: all clean rust
