#include "completion_route.h"
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Packed token/state prevents a stale callback racing an ACK + reuse (ABA).
 * Tokens never wrap or repeat, including across contexts and dispatch threads.
 * The opaque IOKit refcon is never dereferenced. */
enum state { FREE, PENDING, CALLBACK, HELD, RETURNED };
#define STATE_BITS 3u
#define STATE_MASK 7u
#define TOKEN_MAX (UINT64_MAX >> STATE_BITS)
_Static_assert(sizeof(uintptr_t)==8, "64-bit refcon tokens");
_Static_assert(ATOMIC_BOOL_LOCK_FREE==2, "callback flags must be lock-free");
_Static_assert(ATOMIC_LLONG_LOCK_FREE==2, "callback atomics must be lock-free");
struct slot {
    _Atomic uint64_t word;
    uint64_t ticket; /* written before release-PENDING, immutable until ACK */
};
struct ftu_route {
    struct ftu_route_config config;
    struct slot slots[2][FTU_ROUTE_MAX_SLOTS];
    pthread_t owner, thread;
    CFRunLoopRef loop, owner_loop;
    struct ftu_route *claim_next;
    bool start_failed, invalid_source;
    _Atomic bool ready, exit, exited, stopping;
    _Atomic uint64_t callbacks, faults, releases;
    uint64_t acknowledgements, rejections, orphan_start;
    bool in_call, aborted, joined, quarantined, timed_out;
    IOReturn abort_status[2];
};
/* Creation/disposal only. Prevent another route from claiming the same sources
 * or interfaces; quarantined sessions keep their claim. External scheduling
 * outside this adapter remains a caller precondition. No hot-path mutex. */
static pthread_mutex_t claims_lock=PTHREAD_MUTEX_INITIALIZER;
static struct ftu_route *claims;
static bool claim_sources(struct ftu_route *r)
{
    pthread_mutex_lock(&claims_lock);
    for(struct ftu_route *p=claims;p;p=p->claim_next)for(unsigned a=0;a<2;a++)for(unsigned b=0;b<2;b++){
        if(p->config.endpoint[a].source==r->config.endpoint[b].source ||
           p->config.endpoint[a].interface==r->config.endpoint[b].interface){
            pthread_mutex_unlock(&claims_lock);return false;
        }
    }
    r->claim_next=claims;claims=r;pthread_mutex_unlock(&claims_lock);return true;
}
static void unclaim_sources(struct ftu_route *r)
{
    pthread_mutex_lock(&claims_lock);struct ftu_route **p=&claims;
    while(*p && *p!=r)p=&(*p)->claim_next;
    if(*p)*p=r->claim_next;
    pthread_mutex_unlock(&claims_lock);
}
static _Atomic uint64_t token_cursor;
static _Atomic uint64_t orphan_callbacks;
static _Thread_local struct ftu_route *dispatch_route;
static bool owner(struct ftu_route *r) { return r && pthread_equal(r->owner,pthread_self()); }
static uint64_t word(uint64_t token,enum state state) { return (token<<STATE_BITS)|state; }
static uint64_t next_token(void)
{
    uint64_t old=atomic_load(&token_cursor);
    while(old<TOKEN_MAX) if(atomic_compare_exchange_weak(&token_cursor,&old,old+1))return old+1;
    return 0;
}
static uint64_t nanos(void)
{
    struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);
    return (uint64_t)t.tv_sec*1000000000u+(uint64_t)t.tv_nsec;
}
static void pause_wait(void) { struct timespec t={0,100000};nanosleep(&t,NULL); }
static void fault(struct ftu_route *r) { atomic_fetch_add(&r->faults,1);atomic_store(&r->stopping,true); }
static bool valid(struct ftu_route *r,unsigned d,unsigned i) { return owner(r) && d<2 && i<r->config.slots; }
static void routed_callback(void *refcon,IOReturn status,void *arg0)
{
    struct ftu_route *r=dispatch_route;
    uint64_t token=(uintptr_t)refcon;
    if(!r){atomic_fetch_add(&orphan_callbacks,1);return;}
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<r->config.slots;i++){
        struct slot *s=&r->slots[d][i];
        uint64_t expected=word(token,PENDING);
        if((atomic_load_explicit(&s->word,memory_order_acquire)>>STATE_BITS)!=token)continue;
        if(!token || !atomic_compare_exchange_strong_explicit(&s->word,&expected,word(token,CALLBACK),memory_order_acq_rel,memory_order_acquire)){
            fault(r);return;
        }
        atomic_fetch_add(&r->callbacks,1);
        struct ftu_route_endpoint *e=&r->config.endpoint[d];
        const IOUSBLowLatencyIsocFrame *frames=e->frames+i*r->config.entries;
        /* Unknown frame identity is not permission to read or release storage. */
        if(arg0!=frames){fault(r);return;}
        struct ftu_route_event event={.token=token,.ticket=s->ticket,.callback_ticks=mach_absolute_time(),
            .direction=d,.slot=i,.entries=r->config.entries,.status=status,
            .data=e->data+i*r->config.entries*FTU_ROUTE_PACKET_BYTES,.frames=frames};
        atomic_store_explicit(&s->word,word(token,HELD),memory_order_release);
        r->config.complete(r->config.context,r,&event);
        /* Do not touch slot data here: consumer may have released and owner ACKed. */
        return;
    }
    fault(r);
}
static void poll_tick(CFRunLoopTimerRef timer,void *arg)
{
    (void)timer;struct ftu_route *r=arg;
    if(!r->invalid_source && (!CFRunLoopSourceIsValid(r->config.endpoint[0].source) ||
                             !CFRunLoopSourceIsValid(r->config.endpoint[1].source))){
        r->invalid_source=true;fault(r);
    }
    r->config.poll(r->config.context,r,atomic_load(&r->stopping));
    /* Handles a stop which raced entry into CFRunLoopRun. */
    if(atomic_load(&r->exit))CFRunLoopStop(r->loop);
}
static void *route_dispatch_main(void *arg)
{
    struct ftu_route *r=arg;dispatch_route=r;
    r->loop=CFRunLoopGetCurrent();CFRetain(r->loop);
    if(r->config.thread_start && !r->config.thread_start(r->config.context)){r->start_failed=true;goto exit;}
    CFRunLoopTimerContext tc={0};tc.info=r;
    CFRunLoopTimerRef timer=CFRunLoopTimerCreate(NULL,CFAbsoluteTimeGetCurrent()+0.001,0.001,0,0,poll_tick,&tc);
    if(!timer){r->start_failed=true;goto exit;}
    for(unsigned d=0;d<2;d++)CFRunLoopAddSource(r->loop,r->config.endpoint[d].source,kCFRunLoopDefaultMode);
    CFRunLoopAddTimer(r->loop,timer,kCFRunLoopDefaultMode);
    atomic_store_explicit(&r->ready,true,memory_order_release);
    /* One timer allocated at startup; no repeated finite RunInMode timeouts.
     * Timer retries retained publication; complete runs immediately per message. */
    while(!atomic_load(&r->exit))CFRunLoopRun();
    CFRunLoopRemoveTimer(r->loop,timer,kCFRunLoopDefaultMode);
    CFRunLoopTimerInvalidate(timer);CFRelease(timer);
    for(unsigned d=0;d<2;d++)CFRunLoopRemoveSource(r->loop,r->config.endpoint[d].source,kCFRunLoopDefaultMode);
exit:
    dispatch_route=NULL;
    atomic_store_explicit(&r->exited,true,memory_order_release);
    atomic_store_explicit(&r->ready,true,memory_order_release);
    return NULL;
}
struct ftu_route *ftu_route_create(const struct ftu_route_config *c)
{
    if(!c || !c->slots || c->slots>FTU_ROUTE_MAX_SLOTS || !c->entries || c->entries>64 || c->entries%8 ||
       !c->complete || !c->poll || !c->dispose)return NULL;
    for(unsigned d=0;d<2;d++){
        const struct ftu_route_endpoint *e=&c->endpoint[d];
        if(!e->interface || !*e->interface || !e->source || !CFRunLoopSourceIsValid(e->source) ||
           !e->data || !e->frames || !e->pipe ||
           e->data_bytes<(size_t)c->slots*c->entries*FTU_ROUTE_PACKET_BYTES ||
           e->frame_count<(size_t)c->slots*c->entries ||
           !(*e->interface)->AbortPipe || !(*e->interface)->LowLatencyReadIsochPipeAsync ||
           !(*e->interface)->LowLatencyWriteIsochPipeAsync ||
           CFRunLoopContainsSource(CFRunLoopGetCurrent(),e->source,kCFRunLoopDefaultMode) ||
           CFRunLoopContainsSource(CFRunLoopGetCurrent(),e->source,kCFRunLoopCommonModes))return NULL;
    }
    if(c->endpoint[0].source==c->endpoint[1].source)return NULL;
    struct ftu_route *r=calloc(1,sizeof *r);if(!r)return NULL;
    r->config=*c;r->owner=pthread_self();r->orphan_start=atomic_load(&orphan_callbacks);
    /* calloc is not the C11 initialization contract for atomic objects. */
    atomic_init(&r->ready,false);atomic_init(&r->exit,false);atomic_init(&r->exited,false);atomic_init(&r->stopping,false);
    atomic_init(&r->callbacks,0);atomic_init(&r->faults,0);atomic_init(&r->releases,0);
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<c->slots;i++)atomic_init(&r->slots[d][i].word,FREE);
    if(!claim_sources(r)){free(r);return NULL;}
    r->owner_loop=CFRunLoopGetCurrent();CFRetain(r->owner_loop);
    for(unsigned d=0;d<2;d++)CFRetain(c->endpoint[d].source);
    if(pthread_create(&r->thread,NULL,route_dispatch_main,r)){
        for(unsigned d=0;d<2;d++)CFRelease(c->endpoint[d].source);
        CFRelease(r->owner_loop);unclaim_sources(r);free(r);return NULL;
    }
    while(!atomic_load_explicit(&r->ready,memory_order_acquire))pause_wait();
    if(r->start_failed){
        pthread_join(r->thread,NULL);
        for(unsigned d=0;d<2;d++)CFRelease(c->endpoint[d].source);
        CFRelease(r->loop);CFRelease(r->owner_loop);unclaim_sources(r);free(r);return NULL;
    }
    return r;
}
uint8_t *ftu_route_buffer(struct ftu_route *r,unsigned d,unsigned i)
{
    if(!valid(r,d,i) || r->in_call || atomic_load(&r->stopping) ||
       (atomic_load_explicit(&r->slots[d][i].word,memory_order_acquire)&STATE_MASK)!=FREE)return NULL;
    return r->config.endpoint[d].data+i*r->config.entries*FTU_ROUTE_PACKET_BYTES;
}
IOReturn ftu_route_submit(struct ftu_route *r,unsigned d,unsigned i,uint64_t ticket,uint64_t frame,const uint16_t *lengths,uint64_t *token)
{
    if(token)*token=0;
    if(!valid(r,d,i) || !lengths || !token || !ticket)return kIOReturnBadArgument;
    if(r->in_call || atomic_load(&r->stopping))return kIOReturnNotPermitted;
    struct slot *s=&r->slots[d][i];
    if((atomic_load_explicit(&s->word,memory_order_acquire)&STATE_MASK)!=FREE)return kIOReturnBusy;
    for(unsigned j=0;j<r->config.entries;j++)if(lengths[j]>FTU_ROUTE_PACKET_BYTES)return kIOReturnBadArgument;
    uint64_t t=next_token();if(!t){fault(r);return kIOReturnNoResources;}
    struct ftu_route_endpoint *e=&r->config.endpoint[d];
    IOUSBLowLatencyIsocFrame *frames=e->frames+i*r->config.entries;
    for(unsigned j=0;j<r->config.entries;j++){
        memset(&frames[j],0,sizeof frames[j]);frames[j].frReqCount=lengths[j];
        frames[j].frStatus=(IOReturn)kUSBLowLatencyIsochTransferKey;
    }
    s->ticket=ticket;*token=t;r->in_call=true;
    atomic_store_explicit(&s->word,word(t,PENDING),memory_order_release);
    IOReturn result;
    if(d)result=(*e->interface)->LowLatencyWriteIsochPipeAsync(e->interface,e->pipe,e->data+i*r->config.entries*FTU_ROUTE_PACKET_BYTES,frame,r->config.entries,0,frames,routed_callback,(void *)(uintptr_t)t);
    else result=(*e->interface)->LowLatencyReadIsochPipeAsync(e->interface,e->pipe,e->data+i*r->config.entries*FTU_ROUTE_PACKET_BYTES,frame,r->config.entries,0,frames,routed_callback,(void *)(uintptr_t)t);
    if(result){
        uint64_t expected=word(t,PENDING);
        /* A synchronous rejection owes no callback. A callback/error conflict
         * poisons the session; never overwrite the callback's terminal state. */
        if(!atomic_compare_exchange_strong_explicit(&s->word,&expected,word(t,RETURNED),memory_order_acq_rel,memory_order_acquire))fault(r);
        r->rejections++;
    }
    r->in_call=false;return result;
}
bool ftu_route_release(struct ftu_route *r,uint64_t token)
{
    if(!r || dispatch_route!=r || !token || token>TOKEN_MAX)return false;
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<r->config.slots;i++){
        if((atomic_load_explicit(&r->slots[d][i].word,memory_order_acquire)>>STATE_BITS)!=token)continue;
        uint64_t expected=word(token,HELD);
        if(atomic_compare_exchange_strong_explicit(&r->slots[d][i].word,&expected,word(token,RETURNED),memory_order_acq_rel,memory_order_acquire)){
            atomic_fetch_add(&r->releases,1);CFRunLoopWakeUp(r->owner_loop);return true;
        }
    }
    fault(r);return false;
}
bool ftu_route_ack(struct ftu_route *r,unsigned d,unsigned i,uint64_t token)
{
    if(!valid(r,d,i) || r->in_call || !token || token>TOKEN_MAX)return false;
    uint64_t expected=word(token,RETURNED);
    if(!atomic_compare_exchange_strong_explicit(&r->slots[d][i].word,&expected,word(token,FREE),memory_order_acq_rel,memory_order_acquire))return false;
    r->acknowledgements++;return true;
}
void ftu_route_request_stop(struct ftu_route *r) { if(r)atomic_store(&r->stopping,true); }
static bool all_free(struct ftu_route *r)
{
    bool empty=true;
    for(unsigned d=0;d<2;d++)for(unsigned i=0;i<r->config.slots;i++){
        uint64_t v=atomic_load_explicit(&r->slots[d][i].word,memory_order_acquire);
        if((v&STATE_MASK)==RETURNED)ftu_route_ack(r,d,i,v>>STATE_BITS);
        if((atomic_load(&r->slots[d][i].word)&STATE_MASK)!=FREE)empty=false;
    }
    return empty;
}
bool ftu_route_stop(struct ftu_route *r,unsigned timeout_ms)
{
    if(!owner(r))return false;
    ftu_route_request_stop(r);
    if(r->in_call)return false; /* owner serialization must outlive submit/abort */
    if(!r->aborted){
        r->aborted=true;r->in_call=true;
        for(unsigned d=0;d<2;d++){
            struct ftu_route_endpoint *e=&r->config.endpoint[d];
            r->abort_status[d]=(*e->interface)->AbortPipe(e->interface,e->pipe);
            /* Removal is not proof of drain: the same all-free, source-detach,
             * join and poison gates below still have to pass. */
            if(r->abort_status[d] && !ftu_route_device_gone(r->abort_status[d]))fault(r);
        }
        r->in_call=false;
    }
    uint64_t deadline=nanos()+(uint64_t)timeout_ms*1000000u;
    while(!r->joined){
        if(all_free(r))atomic_store(&r->exit,true);
        if(atomic_load_explicit(&r->exited,memory_order_acquire)){
            if(pthread_join(r->thread,NULL)==0)r->joined=true;
            else r->quarantined=true;
            break;
        }
        if(atomic_load(&r->exit)){CFRunLoopStop(r->loop);CFRunLoopWakeUp(r->loop);}
        if(nanos()>=deadline){r->timed_out=true;break;}
        pause_wait();
    }
    if(atomic_load(&r->faults) || atomic_load(&orphan_callbacks)!=r->orphan_start)r->quarantined=true;
    return r->joined && !r->quarantined && all_free(r);
}
bool ftu_route_destroy(struct ftu_route *r)
{
    if(!owner(r))return false;
    if(atomic_load(&orphan_callbacks)!=r->orphan_start || atomic_load(&r->faults))r->quarantined=true;
    if(r->in_call || !r->joined || r->quarantined || !all_free(r))return false;
    /* Drop extra references while the source's backend context still exists.
     * On disposal error the backend preserves any remaining original refs. */
    for(unsigned d=0;d<2;d++)CFRelease(r->config.endpoint[d].source);
    if(!r->config.dispose(r->config.context)){r->quarantined=true;return false;}
    CFRelease(r->loop);CFRelease(r->owner_loop);unclaim_sources(r);free(r);return true;
}
void ftu_route_stats(struct ftu_route *r,struct ftu_route_stats *s)
{
    if(!owner(r) || !s)return;
    memset(s,0,sizeof *s);s->callbacks=atomic_load(&r->callbacks);s->faults=atomic_load(&r->faults);
    s->releases=atomic_load(&r->releases);s->acknowledgements=r->acknowledgements;s->rejections=r->rejections;
    s->stop_requested=atomic_load(&r->stopping);s->joined=r->joined;
    s->ever_timed_out=r->timed_out;
    s->poisoned=r->quarantined || atomic_load(&r->faults) || atomic_load(&orphan_callbacks)!=r->orphan_start;
    s->quarantined=s->poisoned || (r->timed_out && !r->joined);
    for(unsigned d=0;d<2;d++){
        s->abort_status[d]=r->abort_status[d];
        for(unsigned i=0;i<r->config.slots;i++){
            unsigned state=atomic_load(&r->slots[d][i].word)&STATE_MASK;
            if(state==PENDING || state==CALLBACK)s->pending++;
            if(state==HELD)s->held++;
            if(state==RETURNED)s->returned++;
        }
    }
}

void ftu_route_snapshot(struct ftu_route *r,uint64_t w[16])
{
    struct ftu_route_stats s={0};ftu_route_stats(r,&s);
    const uint64_t v[16]={1,s.callbacks,s.faults,s.releases,s.acknowledgements,s.rejections,
        s.pending,s.held,s.returned,s.stop_requested,s.joined,s.quarantined,s.poisoned,
        s.ever_timed_out,(uint64_t)(int64_t)s.abort_status[0],(uint64_t)(int64_t)s.abort_status[1]};
    memcpy(w,v,sizeof v);
}

#ifdef FTU_ROUTE_TEST
/* Isolated-process fault injection; never present in an ordinary object. */
void ftu_route_test_exhaust_tokens(void) { atomic_store(&token_cursor,TOKEN_MAX); }
#endif
