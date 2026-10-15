#ifndef FTU_DUPLEX_H
#define FTU_DUPLEX_H
#include <IOKit/usb/IOUSBLib.h>
#include <signal.h>
#include <stdbool.h>
int duplex_run(IOUSBDeviceInterface500 **dev, const char *output,
               unsigned duration_ms, bool tone, volatile sig_atomic_t *stop);
int stream_run(IOUSBDeviceInterface500 **dev, const char *output,
               unsigned duration_ms, bool tone, unsigned cycles, volatile sig_atomic_t *stop);
#endif
