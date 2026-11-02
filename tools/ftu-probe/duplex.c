/* macOS mechanics only. Scheduling, PCM, feedback and stop/drain policy live
 * in rust/ftu-stream. Live startup permits only a bounded 48 kHz endpoint SET;
 * standalone probes remain GET-only. No mixer writes. */
#include "duplex.h"
#include "../../shared/ftu_native.h"
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOCFPlugIn.h>
#include <mach/mach_time.h>
#include <mach/mach.h>
#include <mach/thread_policy.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <sys/stat.h>
#ifdef FTU_INDEPENDENT
#include "completion_route.h"
#endif

#define MAX_BLOCKS 256u
#define NOT_DONE ((IOReturn)kUSBLowLatencyIsochTransferKey)
struct native;
struct block {
    struct native *owner;
    unsigned direction, index;
    bool used, pending, complete, delivered, matched;
    IOReturn status;
    uint64_t callback_ticks;
    struct ftu_pump pump;
    uint64_t ticket;
    uintptr_t token;
};
struct endpoint {
    IOUSBInterfaceInterface300 **intf;
    CFRunLoopSourceRef source;
    uint8_t *data;
    IOUSBLowLatencyIsocFrame *frames;
    bool opened, original_known, alt_attempted;
    UInt8 original, pipe;
    struct block blocks[MAX_BLOCKS];
};
struct native {
    IOUSBDeviceInterface500 **dev;
    struct endpoint ep[2];
    unsigned count, entries;
    bool pool_mode;
#ifdef FTU_INDEPENDENT
    bool route_attached, route_retired;
    uint64_t shutdown[16];
#endif
    struct ftu_info report;
    uint64_t pump_id, pump_enter, sample_begin, sample_end;
    bool pumping, pump_active;
    struct ftu_pump_observation last_pump;
};
_Static_assert(sizeof(struct ftu_packet) == 16, "packet ABI");
_Static_assert(sizeof(struct ftu_completion) == 1040, "completion ABI");
_Static_assert(sizeof(struct ftu_rates) == 32, "rates ABI");
_Static_assert(sizeof(struct ftu_info) == 68, "info ABI");
_Static_assert(sizeof(struct ftu_clock) == 16, "clock ABI");
_Static_assert(sizeof(struct ftu_pool_completion) == 1104, "pool completion ABI");
_Static_assert(sizeof(struct ftu_transport_geometry)==24,"transport geometry ABI");
_Static_assert(sizeof(struct ftu_pump)==48,"pump ABI");
_Static_assert(sizeof(struct ftu_pump_observation)==80,"all-pump ABI");
_Static_assert(sizeof(struct ftu_worker_sample)==96,"worker sample ABI");
_Static_assert(sizeof(sig_atomic_t) == sizeof(int32_t), "signal ABI");
static uint64_t absolute_ticks(AbsoluteTime t) { uint64_t r; memcpy(&r,&t,sizeof r); return r; }
/* The run loop dispatches on this thread. Opaque tokens are never dereferenced
 * and never reused on this thread, including across its sessions. Separate
 * worker threads have separate run loops and token/context namespaces.
 * A late/duplicate callback cannot
 * inherit a recycled slot's new ticket or touch a retired context. One active
 * pool context per thread is supported by this standalone adapter. */
static _Thread_local struct native *pool_context;
static _Thread_local uintptr_t last_token;
static _Thread_local uint64_t orphan_callbacks;
_Static_assert(sizeof(uintptr_t) == 8, "64-bit callback tokens");
#define TRY(stage_id, call) do { c->report.stage=(stage_id); IOReturn r_=(call); if(r_)return r_; } while(0)
static int32_t config(void *ctx, uint8_t *bytes, size_t capacity, size_t *length)
{
    struct native *c=ctx;
    UInt8 config=0,n=0,speed=0; UInt16 vid=0,pid=0;
    mach_timebase_info_data_t timebase;
    if (mach_timebase_info(&timebase) || !timebase.denom) return kIOReturnError;
    c->report.timebase_numer=timebase.numer; c->report.timebase_denom=timebase.denom;
    TRY(1,(*c->dev)->GetDeviceSpeed(c->dev,&speed));
    TRY(2,(*c->dev)->GetDeviceVendor(c->dev,&vid));
    TRY(3,(*c->dev)->GetDeviceProduct(c->dev,&pid));
    TRY(4,(*c->dev)->GetLocationID(c->dev,&c->report.location));
    c->report.vid=vid;c->report.pid=pid;
    if(speed!=kUSBDeviceSpeedHigh || vid!=0x0763 || (pid!=0x2080 && pid!=0x2081))return kIOReturnUnsupported;
    TRY(5,(*c->dev)->GetConfiguration(c->dev,&config));
    TRY(6,(*c->dev)->GetNumberOfConfigurations(c->dev,&n));
    for(unsigned i=0;i<n;i++) {
        IOUSBConfigurationDescriptorPtr p=NULL;
        TRY(7,(*c->dev)->GetConfigurationDescriptorPtr(c->dev,(UInt8)i,&p));
        if(!p || p->bConfigurationValue!=config)continue;
        const uint8_t *b=(const uint8_t *)p;
        size_t size=b[2]|((size_t)b[3]<<8);
        if(size>capacity || size<9)return kIOReturnBadArgument;
        memcpy(bytes,b,size);*length=size;return 0;
    }
    return kIOReturnNotFound;
}
static void rates(void *ctx,struct ftu_rates *out)
{
    struct native *c=ctx;memset(out,0,sizeof *out);
    for(unsigned i=0;i<2;i++) {
        IOUSBDevRequestTO q={0};q.bmRequestType=0xa2;q.bRequest=0x81;
        q.wValue=0x100;q.wIndex=i?0x81:1;q.wLength=3;q.pData=out->raw+i*3;
        q.noDataTimeout=q.completionTimeout=500;q.wLenDone=UINT32_MAX;
        out->status[i]=(*c->dev)->DeviceRequestTO(c->dev,&q);out->actual[i]=q.wLenDone;
        if(!out->status[i] && q.wLenDone==3)out->hz[i]=out->raw[i*3]|((uint32_t)out->raw[i*3+1]<<8)|((uint32_t)out->raw[i*3+2]<<16);
    }
}
static IOReturn select_interface(struct native *c,unsigned dir)
{
    const int wanted=dir?1:2;struct endpoint *e=&c->ep[dir];
    IOUSBFindInterfaceRequest req={kIOUSBFindInterfaceDontCare,kIOUSBFindInterfaceDontCare,kIOUSBFindInterfaceDontCare,kIOUSBFindInterfaceDontCare};
    io_iterator_t iter=0;TRY(10,(*c->dev)->CreateInterfaceIterator(c->dev,&req,&iter));
    io_service_t chosen=0,s;unsigned count=0;
    while((s=IOIteratorNext(iter))) {
        CFTypeRef p=IORegistryEntryCreateCFProperty(s,CFSTR("bInterfaceNumber"),kCFAllocatorDefault,0);
        int number=-1;bool match=p && CFGetTypeID(p)==CFNumberGetTypeID() && CFNumberGetValue(p,kCFNumberIntType,&number) && number==wanted;
        if(p)CFRelease(p);
        if(match){count++;if(!chosen)chosen=s;else IOObjectRelease(s);}else IOObjectRelease(s);
    }
    bool valid=IOIteratorIsValid(iter);IOObjectRelease(iter);
    if(count!=1 || !valid){if(chosen)IOObjectRelease(chosen);return kIOReturnNotFound;}
    IOCFPlugInInterface **plugin=NULL;SInt32 score=0;
    IOReturn r=IOCreatePlugInInterfaceForService(chosen,kIOUSBInterfaceUserClientTypeID,kIOCFPlugInInterfaceID,&plugin,&score);
    IOObjectRelease(chosen);c->report.stage=11;
    if(r || !plugin){if(plugin)(*plugin)->Release(plugin);return r?r:kIOReturnError;}
    HRESULT h=(*plugin)->QueryInterface(plugin,CFUUIDGetUUIDBytes(kIOUSBInterfaceInterfaceID300),(LPVOID *)&e->intf);
    (*plugin)->Release(plugin);if(h || !e->intf)return kIOReturnError;
    UInt8 number=0;UInt16 vid=0,pid=0;UInt32 location=0,period=0;
    TRY(12,(*e->intf)->GetInterfaceNumber(e->intf,&number));
    TRY(13,(*e->intf)->GetDeviceVendor(e->intf,&vid));
    TRY(14,(*e->intf)->GetDeviceProduct(e->intf,&pid));
    TRY(15,(*e->intf)->GetLocationID(e->intf,&location));
    TRY(16,(*e->intf)->GetFrameListTime(e->intf,&period));
    if(number!=wanted || vid!=c->report.vid || pid!=c->report.pid || location!=c->report.location || period!=125)return kIOReturnUnsupported;
    return 0;
}
static unsigned packet_entries(const struct native *c) { return c->entries?c->entries:FTU_ENTRIES; }
static unsigned block_bytes(const struct native *c) { return packet_entries(c)*312u; }
static bool valid_geometry(const struct ftu_transport_geometry *g)
{
    if(!g || g->version!=1 || (g->entries!=8 && g->entries!=16 && g->entries!=32 && g->entries!=64)
       || g->slots<2 || g->slots>FTU_POOL_MAX_SLOTS || g->replay_ms!=64 || g->pcm_offset!=3072)return false;
    unsigned needed=64/(g->entries/8)+2*g->slots+1,history=1;
    while(history<needed)history*=2;
    return history<=256 && g->history==history;
}
static int32_t allocate_buffers(struct native *c,unsigned d)
{
    struct endpoint *e=&c->ep[d];unsigned count=c->count;
    TRY(23,(*e->intf)->LowLatencyCreateBuffer(e->intf,(void **)&e->data,count*block_bytes(c),d?kUSBLowLatencyWriteBuffer:kUSBLowLatencyReadBuffer));
    TRY(24,(*e->intf)->LowLatencyCreateBuffer(e->intf,(void **)&e->frames,count*packet_entries(c)*sizeof *e->frames,kUSBLowLatencyFrameListBuffer));
    if(!e->data || !e->frames)return kIOReturnNoMemory;
    memset(e->data,0,count*block_bytes(c));memset(e->frames,0,count*packet_entries(c)*sizeof *e->frames);
    for(unsigned i=0;i<count;i++){e->blocks[i].owner=c;e->blocks[i].direction=d;e->blocks[i].index=i;}
    return 0;
}
static int32_t prepare(void *ctx,uint32_t count)
{
    struct native *c=ctx;if(!count || count>MAX_BLOCKS || c->count)return kIOReturnBadArgument;
    c->count=count;
    for(unsigned d=0;d<2;d++) {
        struct endpoint *e=&c->ep[d];IOReturn r=select_interface(c,d);if(r)return r;
        c->report.stage=20;r=(*e->intf)->USBInterfaceOpen(e->intf);
        if(r)return r;e->opened=true;c->report.open_mask|=1u<<d;
        TRY(21,(*e->intf)->GetAlternateSetting(e->intf,&e->original));
        e->original_known=true;c->report.original[d]=e->original;
        if(e->original)return kIOReturnExclusiveAccess;
        UInt8 type=0,interval=0;UInt16 max=0;
        TRY(22,(*e->intf)->GetEndpointProperties(e->intf,1,1,d?kUSBOut:kUSBIn,&type,&max,&interval));
        if(type!=kUSBIsoc || max!=312 || interval!=1)return kIOReturnUnsupported;
        r=allocate_buffers(c,d);if(r)return r;
        TRY(25,(*e->intf)->CreateInterfaceAsyncEventSource(e->intf,&e->source));
        if(!e->source)return kIOReturnError;
        CFRunLoopAddSource(CFRunLoopGetCurrent(),e->source,kCFRunLoopDefaultMode);
    }
    return 0;
}
static int32_t pool_prepare(void *ctx,const struct ftu_transport_geometry *g)
{
    struct native *c=ctx;
    if(!c || c!=pool_context || !c->pool_mode || c->count || c->entries || !valid_geometry(g))return kIOReturnBadArgument;
    c->entries=g->entries;
    return prepare(ctx,g->slots);
}
static int32_t activate(void *ctx)
{
    struct native *c=ctx;
    for(unsigned d=0;d<2;d++) {
        struct endpoint *e=&c->ep[d];if(!e->opened)return kIOReturnNotOpen;
        e->alt_attempted=true;TRY(30,(*e->intf)->SetAlternateInterface(e->intf,1));
        UInt8 alt=0,n=0;TRY(31,(*e->intf)->GetAlternateSetting(e->intf,&alt));
        TRY(32,(*e->intf)->GetNumEndpoints(e->intf,&n));
        if(alt!=1 || n!=(d?2:1))return kIOReturnUnsupported;
        unsigned found=0,extra=0;
        for(UInt8 pipe=1;pipe<=n;pipe++) {
            UInt8 direction=0,number=0,type=0,interval=0;UInt16 max=0;
            TRY(33,(*e->intf)->GetPipeProperties(e->intf,pipe,&direction,&number,&type,&max,&interval));
            if(number==1 && direction==(d?kUSBOut:kUSBIn) && type==kUSBIsoc && max==312 && interval==1){e->pipe=pipe;found++;}
            else if(d && number==2 && direction==kUSBIn && type==kUSBIsoc && max==3 && interval==1)extra++;
            else return kIOReturnUnsupported;
        }
        if(found!=1 || extra!=d)return kIOReturnUnsupported;
        c->report.pipe[d]=e->pipe;
    }
    return 0;
}
static int32_t bus_clock(void *ctx,struct ftu_clock *out)
{
    struct native *c=ctx;AbsoluteTime t;UInt64 frame=0;
    IOReturn r=(*c->ep[0].intf)->GetBusFrameNumber(c->ep[0].intf,&frame,&t);
    if(!r){out->frame=frame;out->ticks=absolute_ticks(t);}return r;
}
static uint8_t *buffer(void *ctx,uint32_t d,uint32_t index)
{
    struct native *c=ctx;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return NULL;
#endif
    if(d>1 || index>=c->count)return NULL;
    struct endpoint *e=&c->ep[d];if(!e->data || e->blocks[index].pending)return NULL;
    return e->data+index*block_bytes(c);
}
/* Called once at callback entry, including rejected duplicate/stale tokens.
 * No packet/frame storage is inspected by this observer. */
static void observe_callback(struct native *c,uint64_t ticks,bool invalid)
{
    if(!c->pump_active)return;
    struct ftu_pump_observation *p=&c->last_pump;
    if(!p->callbacks)p->first_callback_ticks=ticks;
    if(p->callbacks!=UINT64_MAX)p->callbacks++;
    p->last_callback_ticks=ticks;
    if(invalid && p->invalid_callbacks!=UINT64_MAX)p->invalid_callbacks++;
}
static void completed(void *refcon,IOReturn status,void *arg0)
{
    struct block *b=refcon;struct native *c=b->owner;struct endpoint *e=&c->ep[b->direction];
    if(!b->pending || b->complete)c->report.callback_fault=1;
    b->status=status;b->matched=arg0==e->frames+b->index*packet_entries(c);
    if(!b->matched)c->report.callback_fault=1;
    b->pending=false;b->complete=true;b->callback_ticks=mach_absolute_time();
    observe_callback(c,b->callback_ticks,!b->matched);
    b->pump=(struct ftu_pump){0};
    if(c->pumping && c->pump_id)b->pump=(struct ftu_pump){c->pump_id,c->pump_enter,0,0,c->sample_begin,c->sample_end};
}
static void pool_completed(void *refcon,IOReturn status,void *arg0)
{
    struct native *c=pool_context;
    if(!c){if(orphan_callbacks!=UINT64_MAX)orphan_callbacks++;return;}
    uintptr_t token=(uintptr_t)refcon;
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++) {
        struct block *b=&c->ep[d].blocks[i];
        if(b->token==token && token) {
            if(!b->pending || b->complete){observe_callback(c,mach_absolute_time(),true);c->report.callback_fault=1;return;}
            completed(b,status,arg0);return;
        }
    }
    observe_callback(c,mach_absolute_time(),true);c->report.callback_fault=1;
}
static int32_t submit(void *ctx,uint32_t d,uint32_t index,uint64_t frame,const uint16_t *lengths)
{
    struct native *c=ctx;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return kIOReturnNotPermitted;
#endif
    if(d>1 || index>=c->count || !lengths)return kIOReturnBadArgument;
    struct endpoint *e=&c->ep[d];struct block *b=&e->blocks[index];
    if(!e->pipe || b->used)return kIOReturnBadArgument;
    for(unsigned i=0;i<packet_entries(c);i++) {
        if(lengths[i]>312 || (!d && lengths[i]!=312) || (d && lengths[i]%24))return kIOReturnBadArgument;
    }
    for(unsigned i=0;i<packet_entries(c);i++) {
        IOUSBLowLatencyIsocFrame *f=e->frames+index*packet_entries(c)+i;
        memset(f,0,sizeof *f);
        f->frReqCount=lengths[i];f->frActCount=0;f->frStatus=NOT_DONE;
    }
    b->used=true;b->pending=true;
    IOAsyncCallback1 cb=c->pool_mode?pool_completed:completed;
    void *ref=c->pool_mode?(void *)b->token:b;
    IOReturn r=d?(*e->intf)->LowLatencyWriteIsochPipeAsync(e->intf,e->pipe,e->data+index*block_bytes(c),frame,packet_entries(c),0,e->frames+index*packet_entries(c),cb,ref):
        (*e->intf)->LowLatencyReadIsochPipeAsync(e->intf,e->pipe,e->data+index*block_bytes(c),frame,packet_entries(c),0,e->frames+index*packet_entries(c),cb,ref);
    if(r)b->pending=false;return r;
}
static int32_t completion(void *ctx,uint32_t d,uint32_t index,struct ftu_completion *out)
{
    struct native *c=ctx;if(d>1 || index>=c->count || !out)return -1;
    struct endpoint *e=&c->ep[d];struct block *b=&e->blocks[index];
    if(!b->complete || b->delivered)return 0;
    memset(out,0,sizeof *out);out->status=b->status;out->matched=b->matched;out->callback_ticks=b->callback_ticks;
    if(b->matched)for(unsigned i=0;i<packet_entries(c);i++) {
        IOUSBLowLatencyIsocFrame *f=e->frames+index*packet_entries(c)+i;
        out->packets[i]=(struct ftu_packet){f->frStatus,f->frReqCount,f->frActCount,absolute_ticks(f->frTimeStamp)};
    }
    b->delivered=true;return 1;
}
static void pump(void *ctx)
{
    struct native *c=ctx;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return;
#endif
    /* Keep callback identity even when another pump runs before consumption.
     * On ID exhaustion telemetry becomes unavailable; audio still runs. */
    if(c->pump_id!=UINT64_MAX)c->pump_id++;
    c->pumping=c->pump_id!=UINT64_MAX;
    c->last_pump=(struct ftu_pump_observation){0};c->pump_active=true;
    c->pump_enter=mach_absolute_time();
    SInt32 result=CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.002,true);
    uint64_t end=mach_absolute_time();
    if(c->pumping)for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++){
        struct ftu_pump *p=&c->ep[d].blocks[i].pump;
        if(p->id==c->pump_id){p->exit_ticks=end;p->result=(uint64_t)result;}
    }
    c->last_pump.pump=(struct ftu_pump){c->pumping?c->pump_id:0,c->pump_enter,end,(uint64_t)result,c->sample_begin,c->sample_end};
    c->pumping=false;c->pump_active=false;
}
static void last_pump(void *ctx,struct ftu_pump_observation *out)
{
    *out=((struct native *)ctx)->last_pump;
}
static int32_t abort_pipe(void *ctx,uint32_t d)
{
    struct native *c=ctx;if(d>1 || !c->ep[d].pipe)return kIOReturnBadArgument;
    return (*c->ep[d].intf)->AbortPipe(c->ep[d].intf,c->ep[d].pipe);
}
/* Only a joined route with every native/Rust slot returned may tolerate
 * device-removal teardown statuses. Do not interpret a generic error as removal.
 * IOUSBLib owns these buffers: destroy each once, then close/release its interface;
 * never free them manually or retry a pointer after LowLatencyDestroyBuffer. */
static bool cleanup_status(struct native *c,unsigned d,unsigned operation,IOReturn status)
{
#ifdef FTU_INDEPENDENT
    c->shutdown[0]=1;c->shutdown[1]|=1ull<<(d*5+operation);
    c->shutdown[6+d*5+operation]=(uint64_t)(int64_t)status;
    if(c->route_retired && ftu_route_device_gone(status))c->shutdown[2]|=1u<<d;
    if(c->route_retired && (c->shutdown[2]&(1u<<d)) &&
       (ftu_route_device_gone(status) || status==kIOReturnNotOpen || status==MACH_SEND_INVALID_DEST))return true;
#else
    (void)c;(void)d;(void)operation;
#endif
    return status==0;
}
static int32_t cleanup(void *ctx)
{
    struct native *c=ctx;IOReturn first=0;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return FTU_PENDING_OWNERSHIP;
#endif
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++)if(c->ep[d].blocks[i].pending)return FTU_PENDING_OWNERSHIP;
    for(unsigned d=0;d<2;d++) {
        struct endpoint *e=&c->ep[d];IOReturn r=0;
#define SAVE_ERROR(op,expr) do { r=(expr);bool ok=cleanup_status(c,d,op,r);if(r && !c->report.cleanup_status[d])c->report.cleanup_status[d]=r;if(!ok && !first)first=r; } while(0)
        if(e->opened && e->alt_attempted && e->original_known){
            SAVE_ERROR(0,(*e->intf)->SetAlternateInterface(e->intf,e->original));c->report.restore_status[d]=r;
            UInt8 alt=255;SAVE_ERROR(1,(*e->intf)->GetAlternateSetting(e->intf,&alt));
            if(!r && alt==e->original && !c->report.restore_status[d])c->report.restored_mask|=1u<<d;
            else if(!r && alt!=e->original && !first)first=kIOReturnError;
        }
        if(e->source){CFRunLoopRemoveSource(CFRunLoopGetCurrent(),e->source,kCFRunLoopDefaultMode);CFRelease(e->source);e->source=NULL;}
        if(e->data){SAVE_ERROR(2,(*e->intf)->LowLatencyDestroyBuffer(e->intf,e->data));e->data=NULL;}
        if(e->frames){SAVE_ERROR(3,(*e->intf)->LowLatencyDestroyBuffer(e->intf,e->frames));e->frames=NULL;}
        if(e->opened){SAVE_ERROR(4,(*e->intf)->USBInterfaceClose(e->intf));e->opened=false;c->report.open_mask&=~(1u<<d);}
        if(e->intf){(*e->intf)->Release(e->intf);e->intf=NULL;}
#undef SAVE_ERROR
    }
#ifdef FTU_INDEPENDENT
    c->shutdown[3]=c->report.restored_mask;c->shutdown[4]=c->report.open_mask;
    /* A second idempotent cleanup must not erase the first fatal result. */
    if(first)c->shutdown[5]=(uint64_t)(int64_t)first;
#endif
    return first;
}
static void info(void *ctx,struct ftu_info *out){
    struct native *c=ctx;*out=c->report;
    if(c->pool_mode && orphan_callbacks)out->callback_fault=1;
}
static int32_t pool_submit(void *ctx,uint32_t d,uint32_t slot,uint64_t ticket,uint64_t frame,const uint16_t *lengths)
{
    struct native *c=ctx;
    if(!c->pool_mode || pool_context!=c || d>1 || slot>=c->count || !ticket || last_token==UINTPTR_MAX)return kIOReturnBadArgument;
    struct block *b=&c->ep[d].blocks[slot];
    if(b->pending || (b->used && (!b->delivered || !b->matched)))return kIOReturnBusy;
    b->used=b->complete=b->delivered=b->matched=false;
    b->status=0;b->callback_ticks=0;b->ticket=ticket;b->token=++last_token;
    return submit(c,d,slot,frame,lengths);
}
static int32_t pool_completion(void *ctx,uint32_t d,uint32_t slot,struct ftu_pool_completion *out)
{
    struct native *c=ctx;if(d>1 || slot>=c->count || !out)return -1;
    int32_t r=completion(ctx,d,slot,&out->result);
    if(r==1){out->packet_count=packet_entries(c);out->reserved=0;out->ticket=c->ep[d].blocks[slot].ticket;out->pump=c->ep[d].blocks[slot].pump;}
    return r;
}
/* Read-only calling-thread sample. No policy setting, USB access or logging.
 * Both system-call statuses and get_default survive into the diagnostic. */
static void worker_sample(void *ctx,struct ftu_worker_sample *out)
{
    struct native *c=ctx;memset(out,0,sizeof *out);out->begin_ticks=mach_absolute_time();
    thread_port_t thread=mach_thread_self();
    thread_basic_info_data_t cpu={0};mach_msg_type_number_t count=THREAD_BASIC_INFO_COUNT;
    kern_return_t rc=thread_info(thread,THREAD_BASIC_INFO,(thread_info_t)&cpu,&count);
    if(!rc && (count<THREAD_BASIC_INFO_COUNT || cpu.user_time.seconds<0 || cpu.user_time.microseconds<0 || cpu.user_time.microseconds>=1000000 || cpu.system_time.seconds<0 || cpu.system_time.microseconds<0 || cpu.system_time.microseconds>=1000000))rc=KERN_INVALID_ARGUMENT;
    out->cpu_status=(uint64_t)(int64_t)rc;
    if(!rc){out->basic_policy=cpu.policy;out->cpu_user_ns=(uint64_t)cpu.user_time.seconds*1000000000ull+(uint64_t)cpu.user_time.microseconds*1000ull;out->cpu_system_ns=(uint64_t)cpu.system_time.seconds*1000000000ull+(uint64_t)cpu.system_time.microseconds*1000ull;}
    thread_time_constraint_policy_data_t policy={0};boolean_t defaults=FALSE;count=THREAD_TIME_CONSTRAINT_POLICY_COUNT;
    rc=thread_policy_get(thread,THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&policy,&count,&defaults);
    if(!rc && count<THREAD_TIME_CONSTRAINT_POLICY_COUNT)rc=KERN_INVALID_ARGUMENT;
    out->policy_status=(uint64_t)(int64_t)rc;
    if(!rc){out->policy_default=defaults;out->period_ticks=policy.period;out->computation_ticks=policy.computation;out->constraint_ticks=policy.constraint;out->preemptible=policy.preemptible;}
    mach_port_deallocate(mach_task_self(),thread);out->end_ticks=mach_absolute_time();
    c->sample_begin=out->begin_ticks;c->sample_end=out->end_ticks;
}
static uint64_t now_ticks(void *ctx){(void)ctx;return mach_absolute_time();}
int duplex_run(IOUSBDeviceInterface500 **dev,const char *output,unsigned duration_ms,bool tone,volatile sig_atomic_t *stop)
{
    struct native *c=calloc(1,sizeof *c);if(!c)return 1;c->dev=dev;
    const struct ftu_native_ops ops={config,rates,prepare,activate,bus_clock,buffer,submit,completion,pump,abort_pipe,cleanup,info};
    int rc=ftu_duplex_run(&ops,c,output,duration_ms,tone,(const volatile int32_t *)stop);
    /* An adapter/policy accounting mismatch must never free live refcons. */
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++)if(c->ep[d].blocks[i].pending)_Exit(4);
    free(c);return rc;
}
int stream_run(IOUSBDeviceInterface500 **dev,const char *output,unsigned duration_ms,bool tone,unsigned cycles,volatile sig_atomic_t *stop)
{
    if(pool_context || !cycles || cycles>3)return 2;
    for(unsigned cycle=0;cycle<cycles;cycle++) {
        if(*stop)return 3;
        char path[4096];int n=snprintf(path,sizeof path,"%s/session-%03u",output,cycle+1);
        if(n<0 || (size_t)n>=sizeof path || mkdir(path,0700))return 1;
        struct native *c=calloc(1,sizeof *c);if(!c)return 1;c->dev=dev;c->pool_mode=true;pool_context=c;
        const struct ftu_pool_ops ops={FTU_POOL_ABI_VERSION,sizeof(struct ftu_pool_ops),pool_prepare,{config,rates,prepare,activate,bus_clock,buffer,submit,completion,pump,abort_pipe,cleanup,info},pool_submit,pool_completion,now_ticks,worker_sample,last_pump};
        int rc=ftu_stream_run(&ops,c,path,duration_ms,tone,cycle+1,(const volatile int32_t *)stop);
        if(rc==4)_Exit(4);
        for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++)if(c->ep[d].blocks[i].pending)_Exit(4);
        pool_context=NULL;free(c);
        if(rc)return rc;
    }
    return 0;
}

/* Shared native factory for the long-lived Rust session. */
#include "../../shared/ftu_live.h"
/* One read-only registry scan. The IORegistry entry ID is the USB generation:
 * every enumeration (replug, port/hub move) creates a new IOUSBHostDevice entry,
 * even at an unchanged locationID. No serial number exists (iSerialNumber=0). */
static int32_t scan_units(struct ftu_usb_unit *unit,io_service_t *out)
{
    CFMutableDictionaryRef match=IOServiceMatching("IOUSBHostDevice");
    if(!match)return FTU_USB_UNKNOWN;
    io_iterator_t iter=0;
    if(IOServiceGetMatchingServices(kIOMainPortDefault,match,&iter))return FTU_USB_UNKNOWN;
    io_service_t selected=0,s;unsigned count=0;bool identified=true;
    while((s=IOIteratorNext(iter))) {
        const CFStringRef keys[]={CFSTR("idVendor"),CFSTR("idProduct"),CFSTR("locationID")};
        uint32_t values[3]={0};bool have[3]={false,false,false};
        for(unsigned i=0;i<3;i++){
            CFTypeRef p=IORegistryEntryCreateCFProperty(s,keys[i],kCFAllocatorDefault,0);
            have[i]=p && CFGetTypeID(p)==CFNumberGetTypeID() && CFNumberGetValue(p,kCFNumberSInt32Type,&values[i]);
            if(p)CFRelease(p);
        }
        if(have[0] && have[1] && values[0]==0x0763 && (values[1]==0x2080 || values[1]==0x2081)){
            /* A supported unit whose location or generation is unreadable still
             * counts: the scan is UNKNOWN, never a silent ONE for another unit. */
            uint64_t id=0;count++;
            if(!have[2] || IORegistryEntryGetRegistryEntryID(s,&id) || !id)identified=false;
            if(!selected){selected=s;unit->product=values[1];unit->location=values[2];unit->registry_id=id;}
            else IOObjectRelease(s);
        }else IOObjectRelease(s);
    }
    bool valid=IOIteratorIsValid(iter);IOObjectRelease(iter);
    int32_t result=!valid || !identified ? FTU_USB_UNKNOWN : count==0 ? FTU_USB_NONE : count==1 ? FTU_USB_ONE : FTU_USB_AMBIGUOUS;
    if(result==FTU_USB_ONE && out)*out=selected;
    else if(selected)IOObjectRelease(selected);
    return result;
}
static io_service_t find_unit(uint32_t *product,uint32_t *location)
{
    struct ftu_usb_unit unit={0};io_service_t selected=0;
    if(scan_units(&unit,&selected)!=FTU_USB_ONE)return 0;
    *product=unit.product;*location=unit.location;return selected;
}
int32_t ftu_usb_find(uint32_t *product,uint32_t *location)
{
    if(!product || !location)return kIOReturnBadArgument;
    *product=*location=0;io_service_t s=find_unit(product,location);
    if(!s)return kIOReturnNotFound;IOObjectRelease(s);return 0;
}
int32_t ftu_usb_discover(struct ftu_usb_unit *unit)
{
    if(!unit)return FTU_USB_UNKNOWN;
    memset(unit,0,sizeof *unit);
    int32_t result=scan_units(unit,NULL);
    if(result!=FTU_USB_ONE)memset(unit,0,sizeof *unit);
    return result;
}
/* registry!=0 pins the open to one USB enumeration: a replug at the same
 * location (new entry ID) is refused until the HAL detaches and rebinds. The
 * entry ID is checked on the retained service, before any user client exists. */
static void *live_open(uint32_t product,uint32_t location,uint64_t registry)
{
    if(pool_context)return NULL;
    uint32_t actual_product=0,actual_location=0;
    io_service_t s=find_unit(&actual_product,&actual_location);
    if(!s)return NULL;
    if(product!=actual_product || location!=actual_location){IOObjectRelease(s);return NULL;}
    if(registry){
        uint64_t id=0;
        if(IORegistryEntryGetRegistryEntryID(s,&id) || id!=registry){IOObjectRelease(s);return NULL;}
    }
    IOCFPlugInInterface **plugin=NULL;SInt32 score=0;
    IOReturn r=IOCreatePlugInInterfaceForService(s,kIOUSBDeviceUserClientTypeID,kIOCFPlugInInterfaceID,&plugin,&score);
    IOObjectRelease(s);
    if(r || !plugin){if(plugin)(*plugin)->Release(plugin);return NULL;}
    IOUSBDeviceInterface500 **dev=NULL;
    HRESULT h=(*plugin)->QueryInterface(plugin,CFUUIDGetUUIDBytes(kIOUSBDeviceInterfaceID500),(LPVOID *)&dev);
    (*plugin)->Release(plugin);if(h || !dev)return NULL;
    struct native *c=calloc(1,sizeof *c);
    if(!c){(*dev)->Release(dev);return NULL;}
    c->dev=dev;c->pool_mode=true;pool_context=c;return c;
}
static const struct ftu_pool_ops live_operations={FTU_POOL_ABI_VERSION,sizeof(struct ftu_pool_ops),pool_prepare,{config,rates,prepare,activate,bus_clock,buffer,submit,completion,pump,abort_pipe,cleanup,info},pool_submit,pool_completion,now_ticks,worker_sample,last_pump};
static const struct ftu_pool_ops *live_ops(void *ctx){return ctx==pool_context?&live_operations:NULL;}
static int32_t live_close(void *ctx)
{
    struct native *c=ctx;if(!c || pool_context!=c)return kIOReturnBadArgument;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return FTU_PENDING_OWNERSHIP;
#endif
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->count;i++)if(c->ep[d].blocks[i].pending)return FTU_PENDING_OWNERSHIP;
    /* Rust must clean native buffers before dropping the factory context. */
    if(c->ep[0].intf || c->ep[1].intf)return kIOReturnBusy;
    (*c->dev)->Release(c->dev);pool_context=NULL;free(c);return 0;
}
/* Policy values come from Rust; this boundary only converts Mach units and
 * applies/restores scheduling on the calling USB worker. Never affects HAL IO. */
static int32_t live_configure_thread(void *ctx,const struct ftu_thread_schedule *schedule)
{
    if(!ctx || ctx!=pool_context)return kIOReturnBadArgument;
    thread_port_t thread=mach_thread_self();kern_return_t result;
    if(!schedule){
        thread_extended_policy_data_t p={.timeshare=TRUE};
        result=thread_policy_set(thread,THREAD_EXTENDED_POLICY,(thread_policy_t)&p,THREAD_EXTENDED_POLICY_COUNT);
    }else{
        mach_timebase_info_data_t tb;result=mach_timebase_info(&tb);
        if(result || !tb.numer || !tb.denom){mach_port_deallocate(mach_task_self(),thread);return result?result:kIOReturnBadArgument;}
        uint64_t period=(uint64_t)schedule->period_ns*tb.denom/tb.numer;
        uint64_t computation=(uint64_t)schedule->computation_ns*tb.denom/tb.numer;
        uint64_t constraint=(uint64_t)schedule->constraint_ns*tb.denom/tb.numer;
        if(!computation || computation>constraint || constraint>period || period>UINT32_MAX){mach_port_deallocate(mach_task_self(),thread);return kIOReturnBadArgument;}
        thread_time_constraint_policy_data_t p={.period=(uint32_t)period,.computation=(uint32_t)computation,.constraint=(uint32_t)constraint,.preemptible=TRUE};
        result=thread_policy_set(thread,THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&p,THREAD_TIME_CONSTRAINT_POLICY_COUNT);
    }
    mach_port_deallocate(mach_task_self(),thread);return result;
}
/* Worker-only mechanism; Rust selects order and gates admission on readback.
 * Never accept an arbitrary rate, inactive interface or an outstanding transfer. */
static int32_t live_set_rate(void *ctx,uint32_t direction,uint32_t hz,uint32_t *actual)
{
    if(actual)*actual=UINT32_MAX;
    struct native *c=ctx;
    if(!c || c!=pool_context || !actual || direction>1 || hz!=48000 || !c->dev || !c->count)
        return kIOReturnBadArgument;
#ifdef FTU_INDEPENDENT
    if(c->route_attached)return kIOReturnNotPermitted;
#endif
    for(unsigned d=0;d<2;d++){
        struct endpoint *e=&c->ep[d];
        if(!e->opened || !e->intf || !e->alt_attempted || !e->pipe)return kIOReturnNotOpen;
        for(unsigned i=0;i<c->count;i++)if(e->blocks[i].used)return kIOReturnBusy;
    }
    uint8_t bytes[3]={(uint8_t)hz,(uint8_t)(hz>>8),(uint8_t)(hz>>16)};
    IOUSBDevRequestTO q={0};q.bmRequestType=0x22;q.bRequest=0x01;
    q.wValue=0x0100;q.wIndex=direction?0x01:0x81;q.wLength=3;q.pData=bytes;
    q.noDataTimeout=q.completionTimeout=500;q.wLenDone=UINT32_MAX;
    IOReturn status=(*c->dev)->DeviceRequestTO(c->dev,&q);
    *actual=q.wLenDone;return status;
}
const struct ftu_live_factory ftu_usb_factory={live_open,live_ops,live_close,live_configure_thread,live_set_rate};

#ifdef FTU_INDEPENDENT
#include "completion_route.h"
#include <pthread.h>
#include <pthread/qos.h>
#include "../../shared/ftu_independent.h"
struct live_route_context {
    struct native *native;
    struct ftu_independent_hooks hooks;
    struct ftu_route *route;
};
_Static_assert(sizeof(struct ftu_independent_event)==1096,"independent event ABI");
static void live_route_complete(void *ctx,struct ftu_route *route,const struct ftu_route_event *e)
{
    struct live_route_context *c=ctx;
    struct ftu_independent_event out={0};
    out.token=e->token;out.ticket=e->ticket;out.host_ticks=e->callback_ticks;
    out.direction=e->direction;out.slot=e->slot;out.entries=e->entries;
    out.data=e->data;out.bytes=e->entries*312u;
    out.result.status=e->status;out.result.matched=1;out.result.callback_ticks=e->callback_ticks;
    for(unsigned j=0;j<e->entries;j++){
        const IOUSBLowLatencyIsocFrame *f=e->frames+j;
        out.result.packets[j]=(struct ftu_packet){f->frStatus,f->frReqCount,f->frActCount,absolute_ticks(f->frTimeStamp)};
    }
    c->hooks.complete(c->hooks.context,route,&out);
}
static void live_route_poll(void *ctx,struct ftu_route *route,bool stopping)
{
    struct live_route_context *c=ctx;
    c->hooks.poll(c->hooks.context,route,mach_absolute_time(),stopping);
}
static bool live_route_thread_start(void *ctx)
{
    struct live_route_context *c=ctx;
    return c->hooks.completion_qos==1 && pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE,0)==0;
}
static bool live_route_dispose(void *ctx)
{
    struct live_route_context *c=ctx;
    /* Route has joined and released its extra source references. The factory
     * context remains until Rust checks restored alternates/rates and closes. */
    struct ftu_route_stats stats={0};ftu_route_stats(c->route,&stats);
    if(!stats.joined || stats.pending || stats.held || stats.returned || stats.poisoned)return false;
    c->native->route_retired=true;
    for(unsigned d=0;d<2;d++)if(ftu_route_device_gone(stats.abort_status[d]))c->native->shutdown[2]|=1u<<d;
    c->native->route_attached=false;
    if(cleanup(c->native)){c->native->route_attached=true;return false;}
    free(c);return true;
}
void *ftu_live_route_attach(void *ctx,const struct ftu_independent_hooks *hooks)
{
    struct native *c=ctx;
    if(!c || c!=pool_context || c->route_attached || !hooks || !hooks->complete || !hooks->poll || c->count!=12 || c->entries!=32)return NULL;
    for(unsigned d=0;d<2;d++){
        struct endpoint *e=&c->ep[d];
        if(!e->source || !e->pipe || !e->opened || !e->alt_attempted)return NULL;
        for(unsigned i=0;i<c->count;i++)if(e->blocks[i].used)return NULL;
    }
    struct live_route_context *bridge=calloc(1,sizeof *bridge);if(!bridge)return NULL;
    bridge->native=c;bridge->hooks=*hooks;
    struct ftu_route_config cfg={.slots=c->count,.entries=c->entries,.context=bridge,
        .complete=live_route_complete,.poll=live_route_poll,.dispose=live_route_dispose,.thread_start=live_route_thread_start};
    for(unsigned d=0;d<2;d++){
        struct endpoint *e=&c->ep[d];
        CFRunLoopRemoveSource(CFRunLoopGetCurrent(),e->source,kCFRunLoopDefaultMode);
        cfg.endpoint[d]=(struct ftu_route_endpoint){.interface=e->intf,.source=e->source,
            .data=e->data,.frames=e->frames,.pipe=e->pipe,
            .data_bytes=c->count*block_bytes(c),.frame_count=c->count*packet_entries(c)};
    }
    struct ftu_route *route=ftu_route_create(&cfg);
    if(!route){
        for(unsigned d=0;d<2;d++)CFRunLoopAddSource(CFRunLoopGetCurrent(),c->ep[d].source,kCFRunLoopDefaultMode);
        free(bridge);return NULL;
    }
    bridge->route=route;c->route_attached=true;return route;
}
void ftu_live_cleanup_snapshot(void *ctx,uint64_t words[16])
{
    memcpy(words,((struct native *)ctx)->shutdown,16*sizeof *words);
}
uint32_t ftu_live_route_fault(void *route)
{
    struct ftu_route_stats s={0};ftu_route_stats(route,&s);
    return s.poisoned || s.quarantined;
}
#endif
