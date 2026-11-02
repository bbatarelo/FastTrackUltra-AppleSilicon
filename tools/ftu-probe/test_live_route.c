/* Actual live attach/conversion/disposal; injected interfaces, no USB discovery. */
#define FTU_INDEPENDENT 1
#include "duplex.c"
#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <time.h>
struct fake_ep {
    IOUSBInterfaceInterface300 *table; /* IUnknown-compatible self points here. */
    unsigned direction;
    IOUSBInterfaceInterface300 methods;
    CFRunLoopSourceRef source;
    IOAsyncCallback1 callback;
    void *ref;
    IOUSBLowLatencyIsocFrame *frames;
    _Atomic bool pending;
    _Atomic IOReturn status;
};
static pthread_t owner_thread;
static _Atomic uint64_t returned_token;
static _Atomic unsigned callbacks;
static unsigned freed_count,closed_count,restored_count,abort_count;
static bool fail_cleanup;
static IOReturn removal_status;
static bool not_open_after_abort, raw_destroy_error, removed_one, contradict_alt;
static IOReturn removed(void *self){return removed_one && ((struct fake_ep *)self)->direction==1?0:removal_status;}
static void perform(void *ctx)
{
    struct fake_ep *e=ctx;assert(!pthread_equal(owner_thread,pthread_self()));
    if(!atomic_exchange_explicit(&e->pending,false,memory_order_acq_rel))return;
    for(unsigned i=0;i<32;i++){
        e->frames[i].frStatus=e->status;e->frames[i].frActCount=e->status?0:144;
        uint64_t ticks=1000+i;memcpy(&e->frames[i].frTimeStamp,&ticks,sizeof ticks);
    }
    e->callback(e->ref,e->status,e->frames);
}
static IOReturn fake_submit(void *self,UInt8 pipe,void *bytes,UInt64 start,UInt32 count,UInt32 frequency,IOUSBLowLatencyIsocFrame *frames,IOAsyncCallback1 callback,void *ref)
{
    assert(pthread_equal(owner_thread,pthread_self()) && pipe==1 && bytes && start==116 && count==32 && frequency==0);
    struct fake_ep *e=self;assert(!atomic_load(&e->pending));
    e->frames=frames;e->callback=callback;e->ref=ref;e->status=0;atomic_store_explicit(&e->pending,true,memory_order_release);
    return 0;
}
static IOReturn abort_fake(void *self,UInt8 pipe)
{
    struct fake_ep *e=self;assert(pthread_equal(owner_thread,pthread_self()) && pipe==1);abort_count++;
    if(atomic_load_explicit(&e->pending,memory_order_acquire)){e->status=kIOReturnAborted;CFRunLoopSourceSignal(e->source);}
    return removed(self);
}
static IOReturn restore_fake(void *self,UInt8 alt){(void)self;assert(pthread_equal(owner_thread,pthread_self()) && alt==0);restored_count++;return not_open_after_abort?kIOReturnNotOpen:removed(self);}
static IOReturn alt_fake(void *self,UInt8 *alt){(void)self;*alt=contradict_alt?1:0;return contradict_alt?0:not_open_after_abort?kIOReturnNotOpen:removed(self);}
static IOReturn destroy_fake(void *self,void *data){(void)self;assert(pthread_equal(owner_thread,pthread_self()));freed_count++;free(data);return fail_cleanup?kIOReturnError:raw_destroy_error?MACH_SEND_INVALID_DEST:removed(self);}
static IOReturn close_fake(void *self){(void)self;closed_count++;return removed(self);}
static ULONG release_fake(void *self){(void)self;return 0;}
static void on_complete(void *ctx,void *route,const struct ftu_independent_event *e)
{
    assert(ctx==(void *)(uintptr_t)0x1234 && !pthread_equal(owner_thread,pthread_self()));
    qos_class_t qos=QOS_CLASS_UNSPECIFIED;int relative=0;assert(pthread_get_qos_class_np(pthread_self(),&qos,&relative)==0 && qos==QOS_CLASS_USER_INTERACTIVE);
    assert(e->direction==0 && e->slot==0 && e->entries==32 && e->bytes==32*312 && e->ticket==1 && e->result.matched==1 && e->data && e->host_ticks==e->result.callback_ticks);
    for(unsigned i=0;i<32;i++)assert(e->result.packets[i].requested==312 && e->result.packets[i].ticks==1000+i && e->result.packets[i].status==e->result.status);
    for(unsigned i=32;i<64;i++)assert(!e->result.packets[i].actual && !e->result.packets[i].requested);
    assert(ftu_route_release(route,e->token));atomic_store(&returned_token,e->token);atomic_fetch_add(&callbacks,1);
}
static void on_poll(void *ctx,void *route,uint64_t now,uint32_t stopping){(void)route;(void)stopping;assert(ctx==(void *)(uintptr_t)0x1234 && now);}
int main(int argc,char **argv)
{
    assert(argc==2);owner_thread=pthread_self();fail_cleanup=!strcmp(argv[1],"cleanup-failure") || !strcmp(argv[1],"removed-cleanup-error");
    if(!strncmp(argv[1],"removed",7))removal_status=kIOReturnNoDevice;
    if(!strcmp(argv[1],"not-attached"))removal_status=kIOReturnNotAttached;
    not_open_after_abort=!strcmp(argv[1],"removed-not-open");
    raw_destroy_error=!strcmp(argv[1],"removed-raw-destroy");
    removed_one=!strcmp(argv[1],"removed-one");
    not_open_after_abort|=!strcmp(argv[1],"not-open-without-removal");
    raw_destroy_error|=!strcmp(argv[1],"raw-error-without-removal");
    contradict_alt=!strcmp(argv[1],"removed-contradict-alt");
    bool failure=contradict_alt || fail_cleanup || (!removal_status && (not_open_after_abort || raw_destroy_error));
    struct native c={0};c.count=12;c.entries=32;c.pool_mode=true;pool_context=&c;
    struct fake_ep eps[2]={0};
    for(unsigned d=0;d<2;d++){
        struct fake_ep *f=&eps[d];f->direction=d;f->table=&f->methods;atomic_init(&f->pending,false);
        f->methods.LowLatencyReadIsochPipeAsync=fake_submit;f->methods.LowLatencyWriteIsochPipeAsync=fake_submit;f->methods.AbortPipe=abort_fake;
        f->methods.SetAlternateInterface=restore_fake;f->methods.GetAlternateSetting=alt_fake;f->methods.LowLatencyDestroyBuffer=destroy_fake;f->methods.USBInterfaceClose=close_fake;f->methods.Release=release_fake;
        CFRunLoopSourceContext sc={0};sc.info=f;sc.perform=perform;f->source=CFRunLoopSourceCreate(NULL,0,&sc);assert(f->source);
        c.ep[d].intf=&f->table;c.ep[d].source=f->source;c.ep[d].pipe=1;c.ep[d].opened=true;c.ep[d].alt_attempted=true;c.ep[d].original_known=true;
        c.ep[d].data=calloc(12,32*312);c.ep[d].frames=calloc(12*32,sizeof *c.ep[d].frames);assert(c.ep[d].data && c.ep[d].frames);c.report.open_mask|=1u<<d;
        CFRunLoopAddSource(CFRunLoopGetCurrent(),f->source,kCFRunLoopDefaultMode);
    }
    struct ftu_independent_hooks hooks={(void *)(uintptr_t)0x1234,on_complete,on_poll,1};
    if(!strcmp(argv[1],"attach-failure") || !strcmp(argv[1],"policy-failure")){
        if(!strcmp(argv[1],"attach-failure"))eps[1].methods.AbortPipe=NULL;else hooks.completion_qos=0;
        assert(!ftu_live_route_attach(&c,&hooks));assert(!c.route_attached);
        for(unsigned d=0;d<2;d++)assert(CFRunLoopContainsSource(CFRunLoopGetCurrent(),c.ep[d].source,kCFRunLoopDefaultMode));
        assert(cleanup(&c)==0);assert(freed_count==4 && closed_count==2);puts("attach failure retained factory ownership: passed");return 0;
    }
    struct ftu_route *route=ftu_live_route_attach(&c,&hooks);assert(route && c.route_attached);
    assert(!ftu_live_route_attach(&c,&hooks));assert(cleanup(&c)==FTU_PENDING_OWNERSHIP);
    for(unsigned d=0;d<2;d++)assert(!CFRunLoopContainsSource(CFRunLoopGetCurrent(),c.ep[d].source,kCFRunLoopDefaultMode));
    uint16_t lengths[32];for(unsigned i=0;i<32;i++)lengths[i]=312;
    uint64_t token=0;assert(ftu_route_submit(route,0,0,1,116,lengths,&token)==0 && token);
    if(strcmp(argv[1],"abort") && !removal_status){
        CFRunLoopSourceSignal(eps[0].source);
        struct timespec pause={0,1000000};unsigned waited=0;
        while(!atomic_load(&callbacks) && waited++<5000)nanosleep(&pause,NULL);
        assert(atomic_load(&callbacks)==1 && atomic_load(&returned_token)==token);
        assert(ftu_route_ack(route,0,0,token));
    }
    assert(ftu_route_stop(route,1500));assert(abort_count==2 && atomic_load(&callbacks)==1);
    assert(ftu_route_destroy(route)==!failure);
    assert(freed_count==4 && closed_count==2 && restored_count==2);
    assert(c.report.restored_mask==(removed_one?2u:removal_status || not_open_after_abort?0u:3u) && c.report.open_mask==0);
    uint64_t report[16];ftu_live_cleanup_snapshot(&c,report);
    assert(report[0]==1 && report[1]==1023 && report[2]==(removed_one?1u:removal_status?3u:0u));
    assert(report[3]==c.report.restored_mask && report[4]==0);
    assert((int32_t)report[5]==(fail_cleanup || contradict_alt?kIOReturnError:!removal_status && not_open_after_abort?kIOReturnNotOpen:!removal_status && raw_destroy_error?MACH_SEND_INVALID_DEST:0));
    if(removal_status){assert((int32_t)report[10]==removal_status && (int32_t)report[15]==(removed_one?0:removal_status));}
    if(!failure){assert(cleanup(&c)==0);uint64_t again[16];ftu_live_cleanup_snapshot(&c,again);assert(!memcmp(report,again,sizeof report));}
    assert(c.route_attached==failure);
    puts("live attach, conversion and disposal: passed");return 0;
}
