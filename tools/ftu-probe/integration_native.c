#include "integration_native.h"
#include "completion_route.h"
#include <assert.h>
#include <mach/mach.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef NDEBUG
#error "The offline fake requires assertions (including checked Mach operations)."
#endif
#define ENTRIES 32u
struct fixture;
struct request { IOAsyncCallback1 callback;void *ref;IOUSBLowLatencyIsocFrame *frames;uint8_t *data;uint64_t start;_Atomic bool pending; };
struct endpoint {
    IOUSBInterfaceInterface300 *vtable;
    IOUSBInterfaceInterface300 methods;
    struct fixture *f;
    unsigned direction;
    CFMachPortRef port;
    CFRunLoopSourceRef source;
    struct request requests[FTU_ROUTE_MAX_SLOTS];
};
struct fixture {
    struct endpoint ep[2];
    struct ftu_route_config config;
    struct ftu_integrated_hooks hooks;
    _Atomic(struct ftu_route *) route;
    pthread_t owner;
    _Atomic uint64_t now, done, dispatched;
    _Atomic bool stopping;
    _Atomic unsigned fault;
    _Atomic int ppm;
    _Atomic bool physical;
    _Atomic uint64_t physical_start,physical_base;
    uint64_t commands;
};
struct message { mach_msg_header_t h;uint64_t serial,now;uint32_t kind,direction,slot; };
_Static_assert(sizeof(struct ftu_integrated_event)==1096,"portable callback layout");
static void deliver(struct fixture *f,unsigned d,unsigned slot,IOReturn status)
{
    if((f->fault==6 || f->fault==11) && !status)status=kIOReturnNoDevice;
    struct request *q=&f->ep[d].requests[slot];
    assert(atomic_exchange_explicit(&q->pending,false,memory_order_acq_rel));
    for(unsigned j=0;j<ENTRIES;j++){
        q->frames[j].frStatus=status;
        q->frames[j].frActCount=status?0:(d?q->frames[j].frReqCount:(uint16_t)((j%8==0?5:j%8==1?7:6)*24));
        uint64_t ticks=(q->start*1000+(j+1)*125)*24;
        ticks=(uint64_t)((int64_t)ticks+(int64_t)ticks*atomic_load(&f->ppm)/1000000);
        memcpy(&q->frames[j].frTimeStamp,&ticks,sizeof ticks);
        if(!d && !status)for(unsigned k=0;k<q->frames[j].frActCount;k+=3){
            q->data[j*312+k]=0;q->data[j*312+k+1]=0;q->data[j*312+k+2]=0x20;
        }
    }
    if(f->fault==3 && !status)q->frames[0].frActCount=313;
    q->callback(q->ref,status,f->fault==4?(void *)(uintptr_t)1:q->frames);
}
static void perform(CFMachPortRef port,void *bytes,CFIndex size,void *context)
{
    (void)port;struct fixture *f=context;struct message *m=bytes;
    assert(!pthread_equal(f->owner,pthread_self()) && size>=(CFIndex)sizeof *m && m->h.msgh_id==0x465449);
    atomic_store(&f->now,m->now);
    if(m->kind==1)deliver(f,m->direction,m->slot,0);
    if(m->kind==2)deliver(f,m->direction,m->slot,kIOReturnAborted);
    f->hooks.poll(f->hooks.context,f->route,m->now,atomic_load(&f->stopping));
    atomic_fetch_add(&f->dispatched,1);
    if(m->serial)atomic_store_explicit(&f->done,m->serial,memory_order_release);
}
static void completed(void *context,struct ftu_route *route,const struct ftu_route_event *v)
{
    struct fixture *f=context;
    struct ftu_integrated_event e={.token=v->token,.ticket=v->ticket,.logical_us=atomic_load(&f->now),
        .direction=v->direction,.slot=v->slot,.entries=v->entries,.data=v->data,.bytes=v->entries*312};
    if(atomic_load(&f->fault)==8)e.ticket++; /* Rust generation rejection. */
    e.result.status=v->status;e.result.matched=1;e.result.callback_ticks=e.logical_us*24;
    for(unsigned j=0;j<v->entries;j++){
        e.result.packets[j].status=v->frames[j].frStatus;
        e.result.packets[j].requested=v->frames[j].frReqCount;
        e.result.packets[j].actual=v->frames[j].frActCount;
        memcpy(&e.result.packets[j].ticks,(const void *)&v->frames[j].frTimeStamp,sizeof(uint64_t));
    }
    f->hooks.complete(f->hooks.context,route,&e);
}
static uint64_t monotonic_ns(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000000000+t.tv_nsec;}
static void poll_route(void *context,struct ftu_route *route,bool stopping)
{
    struct fixture *f=context;
    /* Streaming time is advanced ONLY by command messages, after HAL service.
     * Real timer ticks keep shutdown alive; they cannot advance virtual work. */
    if(stopping){atomic_store(&f->stopping,true);f->hooks.poll(f->hooks.context,route,atomic_load(&f->now),1);}
    else if(atomic_load_explicit(&f->physical,memory_order_acquire)) {
        uint64_t now=atomic_load(&f->physical_base)+(monotonic_ns()-atomic_load(&f->physical_start))/1000;
        atomic_store(&f->now,now);f->hooks.poll(f->hooks.context,route,now,0);
    }
}
static IOReturn submit(void *self,UInt8 pipe,void *data,UInt64 start,UInt32 count,UInt32 frequency,IOUSBLowLatencyIsocFrame *frames,IOAsyncCallback1 cb,void *ref)
{
    struct endpoint *ep=self;struct fixture *f=ep->f;
    assert(pthread_equal(f->owner,pthread_self()) && pipe==1 && count==ENTRIES && frequency==0);
    unsigned slot=(unsigned)(frames-f->config.endpoint[ep->direction].frames)/ENTRIES;
    assert(slot<f->config.slots);
    if(f->fault==2){f->fault=0;return kIOReturnNoResources;}
    struct request *q=&ep->requests[slot];assert(!atomic_load(&q->pending));
    q->callback=cb;q->ref=ref;q->frames=frames;q->data=data;q->start=start;
    atomic_store_explicit(&q->pending,true,memory_order_release);
    if(f->fault==5){
        /* Lifetime-only fault injection: the fake completes inside this call.
         * It is not an accepted timing scenario and is labelled separately. */
        f->fault=0;
        assert(ftu_fixture_command(f,atomic_load(&f->now),1,ep->direction,slot)==0);
    }
    return 0;
}
static IOReturn abort_pipe(void *self,UInt8 pipe)
{
    struct endpoint *ep=self;struct fixture *f=ep->f;assert(pipe==1 && pthread_equal(f->owner,pthread_self()));
    if(f->fault==1 || f->fault==10)return f->fault==10?kIOReturnNoDevice:0;
    /* Post aborts as actual Mach callbacks; never synthesize owner-side ACKs. */
    for(unsigned i=0;i<f->config.slots;i++)if(atomic_load(&ep->requests[i].pending)){
        struct message m={0};m.h.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
        m.h.msgh_remote_port=CFMachPortGetPort(ep->port);m.h.msgh_size=sizeof m;m.h.msgh_id=0x465449;
        m.serial=0;m.now=atomic_load(&f->now);m.kind=2;m.direction=ep->direction;m.slot=i;
        assert(mach_msg(&m.h,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof m,0,MACH_PORT_NULL,1000,MACH_PORT_NULL)==KERN_SUCCESS);
    }
    return (f->fault==9 || f->fault==11 || (f->fault==12 && ep->direction==0) || (f->fault==13 && ep->direction==1))?kIOReturnNoDevice:0;
}
static bool dispose(void *context)
{
    struct fixture *f=context;
    for(unsigned d=0;d<2;d++){
        CFRelease(f->ep[d].source);CFMachPortInvalidate(f->ep[d].port);CFRelease(f->ep[d].port);
        free(f->config.endpoint[d].data);free(f->config.endpoint[d].frames);
    }
    free(f);return true; /* Rust owner drops its context only after destroy returns. */
}
void *ftu_fixture_create(unsigned slots,const struct ftu_integrated_hooks *hooks)
{
    if(!hooks || !hooks->complete || !hooks->poll || slots<2 || slots>32)return NULL;
    struct fixture *f=calloc(1,sizeof *f);if(!f)return NULL;
    f->owner=pthread_self();f->hooks=*hooks;atomic_init(&f->route,NULL);
    atomic_init(&f->physical,false);atomic_init(&f->physical_base,0);atomic_init(&f->physical_start,0);atomic_init(&f->fault,0);atomic_init(&f->ppm,0);atomic_init(&f->now,100000);atomic_init(&f->done,0);atomic_init(&f->dispatched,0);atomic_init(&f->stopping,false);
    f->config.slots=slots;f->config.entries=ENTRIES;f->config.context=f;
    f->config.complete=completed;f->config.poll=poll_route;f->config.dispose=dispose;
    for(unsigned d=0;d<2;d++){
        struct endpoint *ep=&f->ep[d];ep->vtable=&ep->methods;ep->f=f;ep->direction=d;
        ep->methods.LowLatencyReadIsochPipeAsync=submit;ep->methods.LowLatencyWriteIsochPipeAsync=submit;ep->methods.AbortPipe=abort_pipe;
        for(unsigned i=0;i<slots;i++)atomic_init(&ep->requests[i].pending,false);
        CFMachPortContext c={0};c.info=f;Boolean owned=false;
        ep->port=CFMachPortCreate(NULL,perform,&c,&owned);assert(ep->port && !owned);
        struct mach_port_limits limits={.mpl_qlimit=128};
        assert(mach_port_set_attributes(mach_task_self(),CFMachPortGetPort(ep->port),MACH_PORT_LIMITS_INFO,(mach_port_info_t)&limits,MACH_PORT_LIMITS_INFO_COUNT)==KERN_SUCCESS);
        ep->source=CFMachPortCreateRunLoopSource(NULL,ep->port,0);assert(ep->source);
        f->config.endpoint[d]=(struct ftu_route_endpoint){&ep->vtable,ep->source,calloc(slots,ENTRIES*312),calloc(slots*ENTRIES,sizeof(IOUSBLowLatencyIsocFrame)),1,slots*ENTRIES*312,slots*ENTRIES};
        assert(f->config.endpoint[d].data && f->config.endpoint[d].frames);
    }
    f->route=ftu_route_create(&f->config);
    if(!f->route){dispose(f);return NULL;}return f;
}
uint8_t *ftu_fixture_buffer(void *ctx,unsigned d,unsigned s){return ftu_route_buffer(((struct fixture *)ctx)->route,d,s);}
int32_t ftu_fixture_submit(void *ctx,unsigned d,unsigned s,uint64_t ticket,uint64_t start,const uint16_t *lengths,uint64_t *token)
{
    struct fixture *f=ctx;
    if(f->fault==7){uint16_t bad[ENTRIES];memcpy(bad,lengths,sizeof bad);bad[0]=313;f->fault=0;return ftu_route_submit(f->route,d,s,ticket,start,bad,token);}
    return ftu_route_submit(f->route,d,s,ticket,start,lengths,token);
}
int32_t ftu_fixture_command(void *ctx,uint64_t now,unsigned kind,unsigned d,unsigned slot)
{
    struct fixture *f=ctx;assert(pthread_equal(f->owner,pthread_self()) && kind<=1 && d<2 && slot<f->config.slots);
    struct message m={0};m.h.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
    m.h.msgh_remote_port=CFMachPortGetPort(f->ep[d].port);m.h.msgh_size=sizeof m;m.h.msgh_id=0x465449;
    m.serial=++f->commands;m.now=now;m.kind=kind;m.direction=d;m.slot=slot;
    assert(mach_msg(&m.h,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof m,0,MACH_PORT_NULL,1000,MACH_PORT_NULL)==KERN_SUCCESS);
    struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);uint64_t deadline=(uint64_t)t.tv_sec*1000000000+t.tv_nsec+5000000000ull;
    while(atomic_load_explicit(&f->done,memory_order_acquire)<m.serial){
        clock_gettime(CLOCK_MONOTONIC,&t);if((uint64_t)t.tv_sec*1000000000+t.tv_nsec>deadline)return -2;
        /* Scheduler yield only: no artificial real-time millisecond in virtual tests. */
        sched_yield();
    }
    struct ftu_route_stats stats;ftu_route_stats(f->route,&stats);return stats.poisoned?-3:0;
}
int32_t ftu_fixture_ack(void *ctx,unsigned d,unsigned s,uint64_t token){return ftu_route_ack(((struct fixture *)ctx)->route,d,s,token);}
int32_t ftu_fixture_release(void *route,uint64_t token){return ftu_route_release(route,token);}
void ftu_fixture_fault(void *ctx,unsigned fault){atomic_store(&((struct fixture *)ctx)->fault,fault);}
void ftu_fixture_real_time(void *ctx,uint64_t base){struct fixture *f=ctx;atomic_store(&f->physical_base,base);atomic_store(&f->physical_start,monotonic_ns());atomic_store_explicit(&f->physical,true,memory_order_release);}
void ftu_fixture_clock(void *ctx,int ppm){atomic_store(&((struct fixture *)ctx)->ppm,ppm);}
int32_t ftu_fixture_stop(void *ctx,unsigned timeout)
{struct fixture *f=ctx;atomic_store(&f->stopping,true);return ftu_route_stop(f->route,timeout);}
int32_t ftu_fixture_destroy(void *ctx){return ftu_route_destroy(((struct fixture *)ctx)->route);}
void ftu_fixture_snapshot(void *ctx,uint64_t words[16]) { ftu_route_snapshot(((struct fixture *)ctx)->route,words); }
uint64_t ftu_fixture_stat(void *ctx,unsigned key)
{
    struct fixture *f=ctx;struct ftu_route_stats s;ftu_route_stats(f->route,&s);
    switch(key){case 0:return s.callbacks;case 1:return s.releases;case 2:return s.acknowledgements;case 3:return s.joined;case 4:return s.pending;case 5:return s.held;case 6:return s.poisoned;case 8:return s.rejections;default:return atomic_load(&f->dispatched);}
}
