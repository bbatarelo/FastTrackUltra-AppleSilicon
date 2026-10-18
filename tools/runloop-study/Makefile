CC := clang
CFLAGS := -std=c11 -O2 -Wall -Wextra -Werror
OUT := ../../build/runloop-study

.PHONY: all test clean
all: $(OUT)

$(OUT): main.c
	mkdir -p $(dir $@)
	$(CC) $(CFLAGS) $< -framework CoreFoundation -o $@

test:
	python3 -B -m unittest discover -s . -p 'test_*.py'

clean:
	rm -f $(OUT)
