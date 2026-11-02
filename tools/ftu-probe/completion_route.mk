# Run from repository root: make -f tools/ftu-probe/completion_route.mk test
CC := clang
FLAGS := -std=c11 -O2 -g -Wall -Wextra -Werror -Wno-deprecated-declarations -fno-omit-frame-pointer
OUT ?= build/native-completion
SANITIZER ?= address,undefined
SRC := tools/ftu-probe/completion_route.c tools/ftu-probe/test_completion_route.c
# Instrument allocations made by this boundary, excluding opaque CF/IOKit internals.
AUDIT := -DFTU_ROUTE_TEST -Dcalloc=route_test_calloc -Dmalloc=route_test_malloc -Drealloc=route_test_realloc -Dfree=route_test_free -DCFRunLoopTimerCreate=route_test_timer_create -Dpthread_create=route_test_pthread_create
.PHONY: test build
build: $(OUT)/test-completion
$(OUT)/test-completion: $(SRC) tools/ftu-probe/completion_route.h
	mkdir -p $(OUT)
	$(CC) $(FLAGS) -fsanitize=$(SANITIZER) $(AUDIT) -c tools/ftu-probe/completion_route.c -o $(OUT)/route.o
	$(CC) $(FLAGS) -fsanitize=$(SANITIZER) tools/ftu-probe/test_completion_route.c $(OUT)/route.o -framework CoreFoundation -framework IOKit -o $@
test: build
	python3 tools/ftu-probe/test_completion_route.py $(OUT)/test-completion
