#ifndef FTU_CAPTURE_H
#define FTU_CAPTURE_H
#include <IOKit/usb/IOUSBLib.h>
#include <signal.h>
/* Owns only interface 2 for this call. dev remains owned by the caller.
 * Fatal undrained callback debt terminates this standalone process without
 * freeing in-flight memory. Never call from a long-lived HAL host. */
int capture_run(IOUSBDeviceInterface500 **dev, const char *output,
                unsigned duration_ms, volatile sig_atomic_t *stop_signal);
/* Owns only interface 1; reads IN82, never submits OUT01. Same lifetime rules. */
int feedback_run(IOUSBDeviceInterface500 **dev, const char *output,
                 unsigned duration_ms, volatile sig_atomic_t *stop_signal);
#endif
