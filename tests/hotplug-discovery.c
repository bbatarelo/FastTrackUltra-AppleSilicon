/* Actual native discovery/open code with an injected IOKit registry. No USB. */
#include "../tools/ftu-probe/duplex.h"
#include <IOKit/IOCFPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <string.h>
#include <stdio.h>

struct fake_unit { uint32_t vendor, product, location; uint64_t registry; bool location_valid, registry_valid; };
static struct fake_unit units[3];
static unsigned unit_count, cursor, user_client_attempts;
static bool iterator_valid = true, enumerate_ok = true;
static const io_iterator_t fake_iterator = 99;
static CFMutableDictionaryRef fake_matching(const char *name)
{
    assert(!strcmp(name, "IOUSBHostDevice"));
    return CFDictionaryCreateMutable(NULL, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
}
static kern_return_t fake_services(mach_port_t port, CFDictionaryRef match, io_iterator_t *iterator)
{
    (void)port; CFRelease(match); cursor = 0;
    *iterator = enumerate_ok ? fake_iterator : 0;
    return enumerate_ok ? KERN_SUCCESS : KERN_FAILURE;
}
static io_object_t fake_next(io_iterator_t iterator)
{
    assert(iterator == fake_iterator);
    return cursor < unit_count ? 100 + cursor++ : 0;
}
static struct fake_unit *unit(io_registry_entry_t entry)
{
    assert(entry >= 100 && entry < 100 + unit_count); return &units[entry - 100];
}
static CFTypeRef fake_property(io_registry_entry_t entry, CFStringRef key, CFAllocatorRef allocator, IOOptionBits options)
{
    (void)allocator; (void)options; struct fake_unit *u = unit(entry); uint32_t value;
    if (CFEqual(key, CFSTR("idVendor"))) value = u->vendor;
    else if (CFEqual(key, CFSTR("idProduct"))) value = u->product;
    else { assert(CFEqual(key, CFSTR("locationID"))); if (!u->location_valid) return NULL; value = u->location; }
    return CFNumberCreate(NULL, kCFNumberSInt32Type, &value);
}
static kern_return_t fake_registry(io_registry_entry_t entry, uint64_t *registry)
{
    struct fake_unit *u = unit(entry); *registry = u->registry;
    return u->registry_valid ? KERN_SUCCESS : KERN_FAILURE;
}
static boolean_t fake_valid(io_iterator_t iterator) { assert(iterator == fake_iterator); return iterator_valid; }
static kern_return_t fake_release(io_object_t object) { assert(object == fake_iterator || (object >= 100 && object < 100 + unit_count)); return KERN_SUCCESS; }
static IOReturn fake_user_client(io_service_t service, CFUUIDRef type, CFUUIDRef interface, IOCFPlugInInterface ***plugin, SInt32 *score)
{
    (void)type; (void)interface; (void)score; (void)unit(service);
    user_client_attempts++; *plugin = NULL; return kIOReturnUnsupported;
}
#define IOServiceMatching fake_matching
#define IOServiceGetMatchingServices fake_services
#define IOIteratorNext fake_next
#define IORegistryEntryCreateCFProperty fake_property
#define IORegistryEntryGetRegistryEntryID fake_registry
#define IOIteratorIsValid fake_valid
#define IOObjectRelease fake_release
#define IOCreatePlugInInterfaceForService fake_user_client
#include "../tools/ftu-probe/duplex.c"

int main(void)
{
    struct ftu_usb_unit found;
    assert(ftu_usb_discover(&found) == FTU_USB_NONE);
    units[0] = (struct fake_unit){0x0763, 0x2080, 0x20211100, 800, true, true}; unit_count = 1;
    assert(ftu_usb_discover(&found) == FTU_USB_ONE);
    assert(found.product == 0x2080 && found.location == 0x20211100 && found.registry_id == 800);
    assert(!live_open(0x2080, 0x20211100, 799)); assert(user_client_attempts == 0);
    assert(!live_open(0x2080, 0x02100000, 800)); assert(user_client_attempts == 0);
    assert(!live_open(0x2081, 0x20211100, 800)); assert(user_client_attempts == 0);
    assert(!live_open(0x2080, 0x20211100, 800)); assert(user_client_attempts == 1);
    /* Same port, new enumeration: a previously adopted service must not open it. */
    units[0].registry = 801;
    assert(!live_open(0x2080, 0x20211100, 800)); assert(user_client_attempts == 1);
    assert(ftu_usb_discover(&found) == FTU_USB_ONE && found.registry_id == 801);
    assert(!live_open(0x2080, 0x20211100, 801)); assert(user_client_attempts == 2);
    /* Standalone unpinned mode remains explicit; the HAL never uses registry 0. */
    assert(!live_open(0x2080, 0x20211100, 0)); assert(user_client_attempts == 3);
    units[1] = units[0]; units[1].product = 0x2081; unit_count = 2;
    assert(ftu_usb_discover(&found) == FTU_USB_AMBIGUOUS);
    assert(!live_open(0x2080, 0x20211100, 801)); assert(user_client_attempts == 3);
    units[1].location_valid = false;
    assert(ftu_usb_discover(&found) == FTU_USB_UNKNOWN);
    assert(!live_open(0x2080, 0x20211100, 801)); assert(user_client_attempts == 3);
    units[1].location_valid = true; units[1].registry_valid = false;
    assert(ftu_usb_discover(&found) == FTU_USB_UNKNOWN);
    unit_count = 1; units[0].registry = 0;
    assert(ftu_usb_discover(&found) == FTU_USB_UNKNOWN);
    units[0].registry = 801; iterator_valid = false;
    assert(ftu_usb_discover(&found) == FTU_USB_UNKNOWN);
    iterator_valid = true; enumerate_ok = false;
    assert(ftu_usb_discover(&found) == FTU_USB_UNKNOWN);
    enumerate_ok = true; units[0].vendor = 0x1234;
    assert(ftu_usb_discover(&found) == FTU_USB_NONE);
    assert(found.product == 0 && found.location == 0 && found.registry_id == 0);
    puts("Native hotplug discovery and pinned-open tests passed (no hardware)");
    return 0;
}
