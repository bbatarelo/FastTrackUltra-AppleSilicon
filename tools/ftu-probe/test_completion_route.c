/* Real CFRunLoop dispatch and pthreads; fake IOUSBInterface300 only. No USB open.
 * Fault cases intentionally retain the session, then exit their own process. */
#include "completion_route.h"
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <stddef.h>
#include <mach/mach.h>
#include <unistd.h>
#include <errno.h>

enum mode { NORMAL, EARLY, MANUAL, REJECT, EARLY_ERROR, WRONG_THREAD };
struct message { IOAsyncCallback1 cb; void *ref; IOUSBLowLatencyIsocFrame *frames; void *arg; IOReturn status; uint8_t *data; unsigned entries,direction; };
struct fixture;
struct endpoint {
    IOUSBInterfaceInterface300 *vtable; /* IOUSBLib self points here */
    IOUSBInterfaceInterface300 methods;
    struct fixture *fixture;
    unsigned direction;
    CFRunLoopSourceRef source;
    CFMachPortRef port;
    CFRunLoopRef loop;
    pthread_mutex_t lock;
    struct message queue[128], pending[FTU_ROUTE_MAX_SLOTS];
    unsigned head,tail;
    bool queued[128];
    bool active[FTU_ROUTE_MAX_SLOTS];
};
struct fixture {
    struct endpoint ep[2];
    struct ftu_route_config config;
    struct ftu_route *route;
    pthread_t owner,completion_thread;
    _Atomic unsigned completed, polled, scheduled, canceled, source_releases;
    _Atomic bool hold, allow_release, wrong_ack_checked, stall_callback, callback_entered;
    _Atomic bool request_in_callback;
    uint64_t held[2][FTU_ROUTE_MAX_SLOTS]; /* completion thread only */
    enum mode mode;
    IOReturn removal_status;
    bool lost_abort, abort_error, wrong_arg, duplicate, bad_release, dispose_error, drop_message;
    unsigned alloc_start, free_start;
    unsigned aborts[2];
};
static unsigned disposed;
static bool use_mach=true;
static _Atomic bool fail_timer, fail_thread;
CFRunLoopTimerRef route_test_timer_create(CFAllocatorRef a,CFAbsoluteTime fire,CFTimeInterval interval,CFOptionFlags flags,CFIndex order,CFRunLoopTimerCallBack cb,CFRunLoopTimerContext *ctx)
{
    if(atomic_load(&fail_timer))return NULL;
    return CFRunLoopTimerCreate(a,fire,interval,flags,order,cb,ctx);
}
int route_test_pthread_create(pthread_t *thread,const pthread_attr_t *attr,void *(*start)(void *),void *ctx)
{
    if(atomic_load(&fail_thread))return EAGAIN;
    return pthread_create(thread,attr,start,ctx);
}
static _Atomic unsigned route_allocations, route_frees;
void *route_test_calloc(size_t n,size_t size) {atomic_fetch_add(&route_allocations,1);return calloc(n,size);}
void *route_test_malloc(size_t size) {atomic_fetch_add(&route_allocations,1);return malloc(size);}
void *route_test_realloc(void *p,size_t size) {atomic_fetch_add(&route_allocations,1);return realloc(p,size);}
void route_test_free(void *p) {atomic_fetch_add(&route_frees,1);free(p);}
void ftu_route_test_exhaust_tokens(void);

static uint64_t ns(void) {struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000000000+(uint64_t)t.tv_nsec;}
static void pause_us(unsigned us) {struct timespec t={us/1000000,(us%1000000)*1000};nanosleep(&t,NULL);}
static void wait_unsigned(_Atomic unsigned *v,unsigned count)
{
    uint64_t end=ns()+5000000000ull;
    while(atomic_load(v)<count){assert(ns()<end);pause_us(100);}
}
struct notification { mach_msg_header_t header; uint32_t index; };
static void enqueue(struct endpoint *e,struct message m)
{
    pthread_mutex_lock(&e->lock);unsigned index=0;
    if(e->port){
        while(index<128 && e->queued[index])index++;
        assert(index<128);e->queued[index]=true;e->queue[index]=m;
    }else{assert(e->tail-e->head<128);e->queue[e->tail++%128]=m;}
    CFRunLoopRef loop=e->loop;
    if(!e->port){assert(loop);CFRetain(loop);}pthread_mutex_unlock(&e->lock);
    if(e->port){
        if(e->fixture->drop_message)return;
        struct notification msg={0};msg.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
        msg.header.msgh_size=sizeof msg;msg.header.msgh_remote_port=CFMachPortGetPort(e->port);
        msg.header.msgh_id=0x465455;msg.index=index;
        assert(mach_msg(&msg.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof msg,0,MACH_PORT_NULL,1000,MACH_PORT_NULL)==KERN_SUCCESS);
    }else{CFRunLoopSourceSignal(e->source);CFRunLoopWakeUp(loop);CFRelease(loop);}
}
static const void *source_retain(const void *p) { return p; }
static void source_release(const void *p)
{
    const struct endpoint *e=p;assert(e->fixture->config.slots);
    atomic_fetch_add(&e->fixture->source_releases,1);
}
static void schedule(void *info,CFRunLoopRef loop,CFRunLoopMode mode)
{
    (void)mode;struct endpoint *e=info;assert(!pthread_equal(e->fixture->owner,pthread_self()));
    pthread_mutex_lock(&e->lock);assert(!e->loop);e->loop=loop;pthread_mutex_unlock(&e->lock);
    atomic_fetch_add(&e->fixture->scheduled,1);
}
static void cancel(void *info,CFRunLoopRef loop,CFRunLoopMode mode)
{
    (void)mode;struct endpoint *e=info;assert(!pthread_equal(e->fixture->owner,pthread_self()));
    pthread_mutex_lock(&e->lock);assert(e->loop==loop);e->loop=NULL;pthread_mutex_unlock(&e->lock);
    atomic_fetch_add(&e->fixture->canceled,1);
}
static void dispatch_message(struct message m)
{
    /* Completed DMA: packed packet offsets use requested lengths. Distinct
     * packet patterns expose a mistaken 312-byte stride on nonuniform OUT. */
    if(m.data){
        size_t offset=0;
        for(unsigned j=0;j<m.entries;j++){
            unsigned requested=m.frames[j].frReqCount;
            m.frames[j].frStatus=m.status;m.frames[j].frActCount=m.status?0:requested;
            if(m.direction){for(unsigned k=0;k<requested;k++)assert(m.data[offset+k]==(uint8_t)(0x40+j));}
            else memset(m.data+offset,0x20+j,requested);
            offset+=requested;
        }
    }
    m.cb(m.ref,m.status,m.arg);
}
static void perform(void *info)
{
    struct endpoint *e=info;assert(!pthread_equal(e->fixture->owner,pthread_self()));
    for(;;){
        pthread_mutex_lock(&e->lock);
        if(e->head==e->tail){pthread_mutex_unlock(&e->lock);break;}
        struct message m=e->queue[e->head++%128];pthread_mutex_unlock(&e->lock);
        dispatch_message(m);
    }
}
static void mach_perform(CFMachPortRef port,void *msg,CFIndex size,void *info)
{
    (void)port;struct endpoint *e=info;assert(!pthread_equal(e->fixture->owner,pthread_self()));
    assert(size>=(CFIndex)sizeof(struct notification));struct notification *n=msg;
    assert(n->header.msgh_id==0x465455 && n->index<128);
    pthread_mutex_lock(&e->lock);assert(e->queued[n->index]);
    struct message m=e->queue[n->index];e->queued[n->index]=false;pthread_mutex_unlock(&e->lock);
    dispatch_message(m); /* one message, exactly one callback */
}
static IOReturn fake_submit(void *self,UInt8 pipe,void *data,UInt64 start,UInt32 count,UInt32 frequency,IOUSBLowLatencyIsocFrame *frames,IOAsyncCallback1 cb,void *ref)
{
    struct endpoint *e=self;struct fixture *f=e->fixture;
    assert(pthread_equal(f->owner,pthread_self()) && pipe==1 && start>=100 && frequency==0 && count==f->config.entries);
    unsigned slot=(unsigned)(frames-f->config.endpoint[e->direction].frames)/count;
    assert(slot<f->config.slots && data==f->config.endpoint[e->direction].data+slot*count*312);
    struct message m={cb,ref,frames,frames,0,data,count,e->direction};
    if(f->wrong_arg)m.arg=(void *)(uintptr_t)0x1234;
    if(f->mode==REJECT){
        pthread_mutex_lock(&e->lock);e->pending[slot]=m;pthread_mutex_unlock(&e->lock);
        return kIOReturnNoResources;
    }
    if(f->mode==WRONG_THREAD)cb(ref,0,frames); /* also deliver the real completion */
    pthread_mutex_lock(&e->lock);e->pending[slot]=m;e->active[slot]=f->mode==MANUAL;pthread_mutex_unlock(&e->lock);
    if(f->mode==MANUAL)return 0;
    unsigned before=atomic_load(&f->completed);enqueue(e,m);
    if(f->duplicate){m.data=NULL;enqueue(e,m);}
    if(f->mode==EARLY || f->mode==EARLY_ERROR){
        wait_unsigned(&f->completed,before+1);
        assert(!ftu_route_ack(f->route,e->direction,slot,(uintptr_t)ref)); /* synchronous call still active */
        assert(!ftu_route_buffer(f->route,e->direction,slot));
    }
    return f->mode==EARLY_ERROR?kIOReturnNoResources:0;
}
static IOReturn fake_abort(void *self,UInt8 pipe)
{
    struct endpoint *e=self;struct fixture *f=e->fixture;assert(pipe==1 && pthread_equal(f->owner,pthread_self()));
    f->aborts[e->direction]++;
    for(unsigned i=0;i<f->config.slots;i++){
        pthread_mutex_lock(&e->lock);bool active=e->active[i];struct message m=e->pending[i];e->active[i]=false;pthread_mutex_unlock(&e->lock);
        if(active && !f->lost_abort){m.status=kIOReturnAborted;enqueue(e,m);}
    }
    return f->abort_error?kIOReturnError:f->removal_status;
}
static void complete(void *context,struct ftu_route *r,const struct ftu_route_event *e)
{
    struct fixture *f=context;assert(!pthread_equal(f->owner,pthread_self()));
    f->completion_thread=pthread_self();assert(e->entries==f->config.entries && e->ticket && e->token);
    assert(e->frames==f->config.endpoint[e->direction].frames+e->slot*e->entries);
    if(!e->status){
        size_t offset=0;
        for(unsigned j=0;j<e->entries;j++){
            assert(e->frames[j].frActCount==e->frames[j].frReqCount);
            for(unsigned k=0;k<e->frames[j].frActCount;k++)assert(e->data[offset+k]==(uint8_t)((e->direction?0x40:0x20)+j));
            offset+=e->frames[j].frReqCount;
        }
        for(size_t j=offset;j<e->entries*312;j++)assert(e->data[j]==0x55);
    }
    assert(!ftu_route_ack(r,e->direction,e->slot,e->token)); /* wrong thread */
    assert(!ftu_route_buffer(r,e->direction,e->slot));
    uint16_t bad_lengths[64]={0};uint64_t rejected_token=9;
    assert(ftu_route_submit(r,0,0,1,100,bad_lengths,&rejected_token)==kIOReturnBadArgument && !rejected_token);
    atomic_store(&f->wrong_ack_checked,true);
    atomic_store(&f->callback_entered,true);
    while(atomic_load(&f->stall_callback))pause_us(100);
    if(atomic_load(&f->request_in_callback))ftu_route_request_stop(r);
    if(atomic_load(&f->hold))f->held[e->direction][e->slot]=e->token;
    else {assert(ftu_route_release(r,e->token));if(f->bad_release)assert(!ftu_route_release(r,e->token));}
    atomic_fetch_add(&f->completed,1);
}
static void poll_completion(void *context,struct ftu_route *r,bool stopping)
{
    (void)stopping;struct fixture *f=context;assert(!pthread_equal(f->owner,pthread_self()));
    if(atomic_load(&f->allow_release))for(unsigned d=0;d<2;d++)for(unsigned i=0;i<f->config.slots;i++){
        if(f->held[d][i]){assert(ftu_route_release(r,f->held[d][i]));f->held[d][i]=0;}
    }
    atomic_fetch_add(&f->polled,1);
}
static bool dispose(void *context)
{
    struct fixture *f=context;assert(pthread_equal(f->owner,pthread_self()));assert(atomic_load(&f->canceled)==(use_mach?0u:2u));
    if(f->dispose_error)return false;
    for(unsigned d=0;d<2;d++){
        assert(!f->ep[d].loop);CFRelease(f->ep[d].source);if(f->ep[d].port){CFMachPortInvalidate(f->ep[d].port);CFRelease(f->ep[d].port);}pthread_mutex_destroy(&f->ep[d].lock);
        free(f->config.endpoint[d].data);free(f->config.endpoint[d].frames);
    }
    assert(atomic_load(&f->source_releases)==(use_mach?0u:2u));
    disposed++;free(f);return true;
}
static struct fixture *fixture(unsigned slots,unsigned entries)
{
    struct fixture *f=calloc(1,sizeof *f);assert(f);f->owner=pthread_self();f->config.slots=slots;f->config.entries=entries;
    f->config.context=f;f->config.complete=complete;f->config.poll=poll_completion;f->config.dispose=dispose;
    atomic_init(&f->source_releases,0);atomic_init(&f->completed,0);atomic_init(&f->polled,0);atomic_init(&f->scheduled,0);atomic_init(&f->canceled,0);
    atomic_init(&f->hold,false);atomic_init(&f->allow_release,false);atomic_init(&f->wrong_ack_checked,false);
    atomic_init(&f->stall_callback,false);atomic_init(&f->callback_entered,false);atomic_init(&f->request_in_callback,false);
    for(unsigned d=0;d<2;d++){
        struct endpoint *e=&f->ep[d];e->fixture=f;e->direction=d;e->vtable=&e->methods;
        e->methods.LowLatencyReadIsochPipeAsync=fake_submit;e->methods.LowLatencyWriteIsochPipeAsync=fake_submit;e->methods.AbortPipe=fake_abort;
        pthread_mutex_init(&e->lock,NULL);
        if(use_mach){
            CFMachPortContext c={0};c.info=e;Boolean should_free=false;
            e->port=CFMachPortCreate(NULL,mach_perform,&c,&should_free);assert(e->port && !should_free);
            struct mach_port_limits limits={.mpl_qlimit=128};
            assert(mach_port_set_attributes(mach_task_self(),CFMachPortGetPort(e->port),MACH_PORT_LIMITS_INFO,(mach_port_info_t)&limits,MACH_PORT_LIMITS_INFO_COUNT)==KERN_SUCCESS);
            e->source=CFMachPortCreateRunLoopSource(NULL,e->port,0);
        }else{
            CFRunLoopSourceContext c={0};c.info=e;c.schedule=schedule;c.cancel=cancel;c.perform=perform;c.retain=source_retain;c.release=source_release;
            e->source=CFRunLoopSourceCreate(NULL,0,&c);
        }
        assert(e->source);
        f->config.endpoint[d]=(struct ftu_route_endpoint){&e->vtable,e->source,calloc(slots,entries*312),calloc(slots*entries,sizeof(IOUSBLowLatencyIsocFrame)),1,slots*entries*312,slots*entries};
        assert(f->config.endpoint[d].data && f->config.endpoint[d].frames);
    }
    f->route=ftu_route_create(&f->config);
    if(atomic_load(&fail_timer) || atomic_load(&fail_thread)){
        assert(!f->route && !disposed && !atomic_load(&f->scheduled));
        atomic_store(&fail_timer,false);atomic_store(&fail_thread,false);
        /* Failure returned source/backend ownership and removed registry claim. */
        f->route=ftu_route_create(&f->config);
    }
    assert(f->route && atomic_load(&f->scheduled)==(use_mach?0u:2u));
    f->alloc_start=atomic_load(&route_allocations);f->free_start=atomic_load(&route_frees);return f;
}
static uint64_t submit(struct fixture *f,unsigned d,unsigned i,uint64_t ticket,IOReturn expected)
{
    uint16_t lengths[64];for(unsigned j=0;j<f->config.entries;j++)lengths[j]=d?(uint16_t)(120+24*(j%3)):312;
    uint8_t *data=ftu_route_buffer(f->route,d,i);assert(data);memset(data,0x55,f->config.entries*312);
    size_t offset=0;for(unsigned j=0;j<f->config.entries;j++){memset(data+offset,0x40+j,lengths[j]);offset+=lengths[j];}
    uint64_t token=0;assert(ftu_route_submit(f->route,d,i,ticket,100+ticket*f->config.entries/8,lengths,&token)==expected);assert(token);return token;
}
static void ack(struct fixture *f,unsigned d,unsigned i,uint64_t token)
{
    uint64_t deadline=ns()+5000000000ull;
    while(!ftu_route_ack(f->route,d,i,token)){assert(ns()<deadline);pause_us(100);}
}
static void wait_fault(struct fixture *f)
{
    uint64_t deadline=ns()+5000000000ull;
    for(;;){
        struct ftu_route_stats s;ftu_route_stats(f->route,&s);if(s.faults)return;
        assert(ns()<deadline);pause_us(100);
    }
}
static void finish(struct fixture *f,bool success)
{
    struct ftu_route *r=f->route;
    unsigned allocations=atomic_load(&route_allocations), frees=atomic_load(&route_frees);
    assert(allocations==f->alloc_start && frees==f->free_start); /* no allocation OR free in stream */
    assert(ftu_route_stop(r,1000)==success);struct ftu_route_stats s;ftu_route_stats(r,&s);
    assert(f->aborts[0]==1 && f->aborts[1]==1);if(success)assert(s.joined);
    assert(ftu_route_stop(r,1000)==success && f->aborts[0]==1 && f->aborts[1]==1);
    printf("callbacks=%llu releases=%llu acks=%llu faults=%llu pending=%u held=%u joined=%d quarantined=%d\n",s.callbacks,s.releases,s.acknowledgements,s.faults,s.pending,s.held,s.joined,s.quarantined);
    assert(atomic_load(&route_allocations)==allocations && atomic_load(&route_frees)==frees);
    assert(ftu_route_destroy(r)==success);assert(disposed==(unsigned)success);
    assert(atomic_load(&route_allocations)==allocations && atomic_load(&route_frees)==frees+(unsigned)success);
}
static void normal_exchange(unsigned slots,unsigned entries,unsigned count)
{
    struct fixture *f=fixture(slots,entries);
    for(unsigned lap=0;lap<count;lap++){
        uint64_t tokens[2][FTU_ROUTE_MAX_SLOTS];
        for(unsigned d=0;d<2;d++)for(unsigned i=0;i<slots;i++)tokens[d][i]=submit(f,d,i,1+lap*slots+i,0);
        /* Reverse ACK order is permitted; callback identity is independent of slot order. */
        for(unsigned d=0;d<2;d++)for(unsigned i=slots;i-->0;)ack(f,d,i,tokens[d][i]);
    }
    wait_unsigned(&f->completed,2*slots*count);assert(atomic_load(&f->wrong_ack_checked));finish(f,true);
}
int main(int argc,char **argv)
{
    assert(argc==2);alarm(25);const char *test=argv[1];
    if(!strcmp(test,"source-v0")){use_mach=false;normal_exchange(12,32,4);return 0;}
    if(!strcmp(test,"stress")){normal_exchange(12,32,1000);return 0;}
    if(!strcmp(test,"geometry")){normal_exchange(32,8,4);return 0;}
    if(!strcmp(test,"baseline")){normal_exchange(4,64,4);return 0;}
    if(!strcmp(test,"ten")){normal_exchange(10,32,4);return 0;}
    if(!strcmp(test,"timer-failure"))atomic_store(&fail_timer,true);
    if(!strcmp(test,"thread-failure"))atomic_store(&fail_thread,true);
    struct fixture *f=fixture(12,32);
    if(!strcmp(test,"timer-failure") || !strcmp(test,"thread-failure")){finish(f,true);}
    else if(!strcmp(test,"early")){f->mode=EARLY;uint64_t t=submit(f,0,0,1,0);ack(f,0,0,t);finish(f,true);}
    else if(!strcmp(test,"reject")){f->mode=REJECT;uint64_t t=submit(f,1,0,1,kIOReturnNoResources);ack(f,1,0,t);assert(!atomic_load(&f->completed));finish(f,true);}
    else if(!strcmp(test,"early-error")){f->mode=EARLY_ERROR;submit(f,0,0,1,kIOReturnNoResources);finish(f,false);}
    else if(!strcmp(test,"reject-late")){
        f->mode=REJECT;uint64_t old=submit(f,0,0,1,kIOReturnNoResources);ack(f,0,0,old);
        struct message stale=f->ep[0].pending[0];stale.data=NULL;
        f->mode=MANUAL;uint64_t now=submit(f,0,0,2,0);assert(now!=old);
        enqueue(&f->ep[0],stale);wait_fault(f);
        struct ftu_route_stats stats;ftu_route_stats(f->route,&stats);assert(stats.pending==1 && !stats.callbacks);
        finish(f,false);
    }
    else if(!strcmp(test,"token-exhaustion")){
        ftu_route_test_exhaust_tokens();uint16_t lengths[64]={0};uint64_t t=999;
        assert(ftu_route_submit(f->route,0,0,1,100,lengths,&t)==kIOReturnNoResources && !t);
        finish(f,false);
    }
    else if(!strcmp(test,"invalid")){
        struct ftu_route_config c=f->config;c.slots=33;assert(!ftu_route_create(&c));
        c=f->config;c.entries=7;assert(!ftu_route_create(&c));
        c=f->config;c.endpoint[0].data_bytes--;assert(!ftu_route_create(&c));
        c=f->config;c.endpoint[1].frame_count--;assert(!ftu_route_create(&c));
        c=f->config;c.endpoint[1].source=c.endpoint[0].source;assert(!ftu_route_create(&c));
        assert(!ftu_route_create(&f->config)); /* cannot double-claim sources on another loop */
        f->alloc_start=atomic_load(&route_allocations);f->free_start=atomic_load(&route_frees);
        uint16_t lengths[64]={313};uint64_t t=9;
        assert(ftu_route_submit(f->route,0,0,1,100,lengths,&t)==kIOReturnBadArgument && !t);
        t=9;assert(ftu_route_submit(f->route,0,0,0,100,lengths,&t)==kIOReturnBadArgument && !t);
        t=9;assert(ftu_route_submit(f->route,0,32,1,100,lengths,&t)==kIOReturnBadArgument && !t);
        assert(!ftu_route_buffer(f->route,2,0) && !ftu_route_buffer(f->route,0,32));
        uint64_t valid=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);
        assert(!ftu_route_ack(f->route,0,0,valid+(UINT64_C(1)<<61)));
        ack(f,0,0,valid);finish(f,true);
    }
    else if(!strcmp(test,"abort")){f->mode=MANUAL;for(unsigned d=0;d<2;d++)for(unsigned i=0;i<12;i++)submit(f,d,i,i+1,0);finish(f,true);}
    else if(!strcmp(test,"lost-abort")){f->mode=MANUAL;f->lost_abort=true;submit(f,0,0,1,0);assert(!ftu_route_stop(f->route,10));pause_us(10000);finish(f,false);}
    else if(!strcmp(test,"removed") || !strcmp(test,"not-attached") || !strcmp(test,"removed-lost") || !strcmp(test,"removed-poison")){
        f->mode=MANUAL;f->removal_status=!strcmp(test,"not-attached")?kIOReturnNotAttached:kIOReturnNoDevice;
        f->lost_abort=!strcmp(test,"removed-lost");f->wrong_arg=!strcmp(test,"removed-poison");
        for(unsigned d=0;d<2;d++)for(unsigned i=0;i<12;i++)submit(f,d,i,i+1,0);
        bool success=!f->lost_abort && !f->wrong_arg;
        assert(ftu_route_stop(f->route,100)==success);
        uint64_t w[16];ftu_route_snapshot(f->route,w);assert(w[0]==1 && w[9]);
        if(success){assert(w[1]==24 && w[3]==24 && w[4]==24 && w[10] && !w[6] && !w[7] && !w[8] && !w[12]);assert((IOReturn)w[14]==f->removal_status);}
        else if(f->lost_abort){assert(w[6]==24 && !w[10]);}
        else { assert(w[12] && (IOReturn)w[14]==kIOReturnNoDevice); }
        finish(f,success);
    }
    else if(!strcmp(test,"abort-error")){f->abort_error=true;finish(f,false);}
    else if(!strcmp(test,"hold")){
        atomic_store(&f->hold,true);uint64_t t=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);
        assert(!ftu_route_buffer(f->route,0,0) && !ftu_route_ack(f->route,0,0,t));
        atomic_store(&f->allow_release,true);finish(f,true);
    }
    else if(!strcmp(test,"lost-release")){
        atomic_store(&f->hold,true);submit(f,0,0,1,0);wait_unsigned(&f->completed,1);
        assert(!ftu_route_stop(f->route,10));pause_us(10000);finish(f,false);
    }
    else if(!strcmp(test,"stalled-consumer")){
        atomic_store(&f->stall_callback,true);submit(f,0,0,1,0);
        while(!atomic_load(&f->callback_entered))pause_us(100);
        assert(!ftu_route_stop(f->route,10));struct ftu_route_stats s;ftu_route_stats(f->route,&s);
        assert(!s.joined && s.quarantined && !ftu_route_destroy(f->route));
        atomic_store(&f->stall_callback,false);wait_unsigned(&f->completed,1);finish(f,true);
    }
    else if(!strcmp(test,"wrong-thread")){
        f->mode=WRONG_THREAD;uint64_t t=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);ack(f,0,0,t);
        assert(!ftu_route_stop(f->route,1000));struct ftu_route_stats state;ftu_route_stats(f->route,&state);
        assert(state.joined && state.poisoned && !state.pending && !state.faults);finish(f,false);
    }
    else if(!strcmp(test,"wrong-frame")){f->wrong_arg=true;submit(f,0,0,1,0);wait_fault(f);assert(!ftu_route_stop(f->route,10));pause_us(10000);finish(f,false);}
    else if(!strcmp(test,"duplicate")){f->duplicate=true;submit(f,0,0,1,0);wait_fault(f);finish(f,false);}
    else if(!strcmp(test,"bad-release")){f->bad_release=true;submit(f,0,0,1,0);wait_unsigned(&f->completed,1);finish(f,false);}
    else if(!strcmp(test,"stale")){
        uint64_t old=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);ack(f,0,0,old);
        struct message stale=f->ep[0].pending[0];stale.data=NULL;
        f->mode=MANUAL;uint64_t now=submit(f,0,0,2,0);assert(now!=old && !ftu_route_ack(f->route,0,0,old));
        enqueue(&f->ep[0],stale);wait_fault(f);finish(f,false);
    }
    else if(!strcmp(test,"post-destroy")){
        uint64_t t=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);ack(f,0,0,t);
        struct message stale=f->ep[0].pending[0];finish(f,true);
        stale.cb(stale.ref,0,stale.frames); /* opaque token; no retired pointer dereference */
    }
    else if(!strcmp(test,"cross-session")){
        uint64_t t=submit(f,0,0,1,0);wait_unsigned(&f->completed,1);ack(f,0,0,t);
        struct message stale=f->ep[0].pending[0];stale.data=NULL;finish(f,true);disposed=0;
        f=fixture(12,32);f->mode=MANUAL;uint64_t now=submit(f,0,0,1,0);assert(now!=t);
        enqueue(&f->ep[0],stale);wait_fault(f);finish(f,false);
    }
    else if(!strcmp(test,"dispose-error")){
        assert(ftu_route_stop(f->route,1000));f->dispose_error=true;
        assert(!ftu_route_destroy(f->route) && !disposed);
        struct ftu_route_stats s;ftu_route_stats(f->route,&s);assert(s.joined && s.quarantined);
        assert(!ftu_route_destroy(f->route) && !disposed); /* never repeats partial cleanup */
    }
    else if(!strcmp(test,"zero-timeout")){
        /* A held slot makes the timeout deterministic; the retry must recover. */
        atomic_store(&f->hold,true);submit(f,0,0,1,0);wait_unsigned(&f->completed,1);
        assert(!ftu_route_stop(f->route,0));struct ftu_route_stats state;ftu_route_stats(f->route,&state);
        assert(state.ever_timed_out && !state.joined && !state.poisoned && !state.faults);
        atomic_store(&f->allow_release,true);finish(f,true);
    }
    else if(!strcmp(test,"late-abort")){
        f->mode=MANUAL;f->lost_abort=true;submit(f,0,0,1,0);
        assert(!ftu_route_stop(f->route,1) && !ftu_route_destroy(f->route));
        struct message late=f->ep[0].pending[0];late.status=kIOReturnAborted;
        enqueue(&f->ep[0],late);wait_unsigned(&f->completed,1);finish(f,true);
    }
    else if(!strcmp(test,"dead-source")){
        f->mode=MANUAL;submit(f,0,0,1,0);f->lost_abort=true;
        for(unsigned d=0;d<2;d++)CFMachPortInvalidate(f->ep[d].port);
        wait_fault(f);unsigned before=atomic_load(&f->polled);pause_us(50000);
        assert(atomic_load(&f->polled)-before<300);finish(f,false);
    }
    else if(!strcmp(test,"lost-message")){
        f->drop_message=true;submit(f,0,0,1,0);assert(!ftu_route_stop(f->route,1));
        struct ftu_route_stats state;ftu_route_stats(f->route,&state);
        assert(state.pending==1 && state.ever_timed_out && !state.faults && !state.poisoned);
        finish(f,false);
    }
    else if(!strcmp(test,"stop-admission")){
        atomic_store(&f->request_in_callback,true);submit(f,0,0,1,0);wait_unsigned(&f->completed,1);
        uint16_t lengths[64]={0};uint64_t t=0;
        assert(ftu_route_submit(f->route,1,0,2,100,lengths,&t)==kIOReturnNotPermitted && !t);
        assert(!ftu_route_buffer(f->route,1,0));finish(f,true);
    }
    else assert(!"unknown test");
    return 0;
}
