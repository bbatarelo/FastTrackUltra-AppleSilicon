/* No hardware: test the actual callback/token adapter with fake IOUSBLib submits. */
#include "duplex.c"
#include <assert.h>
static IOAsyncCallback1 saved_cb;
static void *saved_ref;
static IOUSBLowLatencyIsocFrame *saved_frames;
static IOReturn fake_submit(void *self,UInt8 pipe,void *buf,UInt64 start,UInt32 count,UInt32 frequency,IOUSBLowLatencyIsocFrame *frames,IOAsyncCallback1 callback,void *ref)
{
    (void)self;(void)pipe;(void)buf;(void)start;(void)frequency;
    assert(count==64);saved_cb=callback;saved_ref=ref;saved_frames=frames;return 0;
}
static void three_slot_native_boundary(void)
{
    struct native c={0};c.count=3;c.pool_mode=true;pool_context=&c;
    IOUSBInterfaceInterface300 table={0};table.LowLatencyWriteIsochPipeAsync=fake_submit;table.LowLatencyReadIsochPipeAsync=fake_submit;
    IOUSBInterfaceInterface300 *ptr=&table;
    uint16_t lengths[FTU_ENTRIES];struct ftu_pool_completion done;
    for(unsigned d=0;d<2;d++){
        c.ep[d].intf=&ptr;c.ep[d].pipe=1;
        c.ep[d].data=calloc(c.count,FTU_BLOCK_BYTES);
        c.ep[d].frames=calloc(c.count*FTU_ENTRIES,sizeof *c.ep[d].frames);
        for(unsigned i=0;i<FTU_ENTRIES;i++)lengths[i]=d?144:312;
        for(unsigned slot=0;slot<c.count;slot++){
            struct block *b=&c.ep[d].blocks[slot];b->owner=&c;b->direction=d;b->index=slot;
            assert(pool_submit(&c,d,slot,slot+1,100+8*slot,lengths)==0);
            assert(saved_frames==c.ep[d].frames+slot*FTU_ENTRIES);
            assert(buffer(&c,d,slot)==NULL);
            saved_cb(saved_ref,0,saved_frames);
            assert(pool_completion(&c,d,slot,&done)==1 && done.ticket==slot+1);
            assert(buffer(&c,d,slot)==c.ep[d].data+slot*FTU_BLOCK_BYTES);
        }
        assert(pool_submit(&c,d,3,4,124,lengths)!=0);
        assert(pool_completion(&c,d,3,&done)==-1 && buffer(&c,d,3)==NULL);
        free(c.ep[d].data);free(c.ep[d].frames);
    }
    pool_context=NULL;
}

static void dispatch_saved(void *info) {(void)info;saved_cb(saved_ref,0,saved_frames);}
static void callback_pump_link(void)
{
    struct native c={0};c.count=4;c.pool_mode=true;pool_context=&c;
    IOUSBInterfaceInterface300 table={0};table.LowLatencyWriteIsochPipeAsync=fake_submit;
    IOUSBInterfaceInterface300 *ptr=&table;c.ep[1].intf=&ptr;c.ep[1].pipe=1;
    c.ep[1].data=calloc(4,FTU_BLOCK_BYTES);c.ep[1].frames=calloc(4*FTU_ENTRIES,sizeof *c.ep[1].frames);
    struct block *b=&c.ep[1].blocks[0];b->owner=&c;b->direction=1;b->index=0;
    CFRunLoopSourceContext sc={0};sc.perform=dispatch_saved;
    CFRunLoopSourceRef source=CFRunLoopSourceCreate(NULL,0,&sc);assert(source);
    CFRunLoopAddSource(CFRunLoopGetCurrent(),source,kCFRunLoopDefaultMode);
    uint16_t lengths[FTU_ENTRIES];for(unsigned i=0;i<FTU_ENTRIES;i++)lengths[i]=144;
    assert(pool_submit(&c,1,0,1,100,lengths)==0);
    CFRunLoopSourceSignal(source);pump(&c);
    assert(b->pump.id==1 && b->pump.enter_ticks<=b->callback_ticks && b->callback_ticks<=b->pump.exit_ticks);
    assert(b->pump.result==kCFRunLoopRunHandledSource);
    struct ftu_pump original=b->pump;
    pump(&c);assert(c.pump_id==2);
    struct ftu_pool_completion done={0};assert(pool_completion(&c,1,0,&done)==1);
    assert(memcmp(&done.pump,&original,sizeof original)==0);
    assert(pool_submit(&c,1,0,2,108,lengths)==0);
    /* Out-of-pump completion cannot inherit an earlier generation's link. */
    saved_cb(saved_ref,0,saved_frames);assert(pool_completion(&c,1,0,&done)==1);
    assert(done.pump.id==0 && done.pump.exit_ticks==0);
    c.pump_id=UINT64_MAX-1;
    assert(pool_submit(&c,1,0,3,116,lengths)==0);CFRunLoopSourceSignal(source);pump(&c);
    assert(pool_completion(&c,1,0,&done)==1 && done.pump.id==0);
    struct ftu_worker_sample a,b_sample;worker_sample(&c,&a);worker_sample(&c,&b_sample);
    assert(!a.cpu_status && !b_sample.cpu_status && !a.policy_status);
    assert(a.end_ticks>=a.begin_ticks && b_sample.begin_ticks>=a.end_ticks);
    assert(b_sample.cpu_user_ns>=a.cpu_user_ns && b_sample.cpu_system_ns>=a.cpu_system_ns);
    mach_timebase_info_data_t tb;assert(mach_timebase_info(&tb)==0);
    uint64_t sum=0,max=0;
    for(unsigned i=0;i<100;i++){worker_sample(&c,&a);uint64_t ns=(a.end_ticks-a.begin_ticks)*tb.numer/tb.denom;sum+=ns;if(ns>max)max=ns;}
    printf("native sampler test-thread mean_ns=%llu max_ns=%llu (100 samples; no audio)\n",(unsigned long long)(sum/100),(unsigned long long)max);
    CFRunLoopRemoveSource(CFRunLoopGetCurrent(),source,kCFRunLoopDefaultMode);CFRelease(source);
    free(c.ep[1].data);free(c.ep[1].frames);pool_context=NULL;
}
int main(void)
{
    struct native c={0};c.count=4;c.pool_mode=true;pool_context=&c;
    IOUSBInterfaceInterface300 table={0};table.LowLatencyWriteIsochPipeAsync=fake_submit;table.LowLatencyReadIsochPipeAsync=fake_submit;
    IOUSBInterfaceInterface300 *ptr=&table;
    c.ep[1].intf=&ptr;c.ep[1].pipe=1;c.ep[1].data=calloc(4,FTU_BLOCK_BYTES);c.ep[1].frames=calloc(4*64,sizeof *c.ep[1].frames);
    struct block *b=&c.ep[1].blocks[0];b->owner=&c;b->direction=1;b->index=0;
    uint16_t lengths[64];for(unsigned i=0;i<64;i++)lengths[i]=144;
    assert(pool_submit(&c,1,0,1,100,lengths)==0);void *old_ref=saved_ref;
    assert(pool_submit(&c,1,0,2,108,lengths)==kIOReturnBusy);
    assert(cleanup(&c)==FTU_PENDING_OWNERSHIP);
    saved_cb(saved_ref,0,saved_frames);
    assert(pool_submit(&c,1,0,2,108,lengths)==kIOReturnBusy);
    struct ftu_pool_completion completion;
    assert(pool_completion(&c,1,0,&completion)==1 && completion.ticket==1);
    assert(pool_completion(&c,1,0,&completion)==0);
    assert(pool_submit(&c,1,0,2,108,lengths)==0 && saved_ref!=old_ref);
    saved_cb(old_ref,0,saved_frames);assert(c.report.callback_fault && b->pending && !b->complete);
    saved_cb(saved_ref,0,saved_frames);assert(!b->pending && b->complete);
    assert(pool_completion(&c,1,0,&completion)==1 && completion.ticket==2);
    c.report.callback_fault=0;pool_context=NULL;saved_cb(old_ref,0,saved_frames);assert(!c.report.callback_fault && orphan_callbacks==1);
    struct ftu_info report;info(&c,&report);assert(report.callback_fault);orphan_callbacks=0;
    /* Simulate a fresh native context: a previous session token is still stale. */
    pool_context=&c;assert(pool_submit(&c,1,0,1,200,lengths)==0);
    saved_cb(old_ref,0,saved_frames);assert(c.report.callback_fault && b->pending);
    saved_cb(saved_ref,0,saved_frames);assert(pool_completion(&c,1,0,&completion)==1);
    /* IN and mismatched frame-list paths use the same strict token ownership. */
    c.ep[0].intf=&ptr;c.ep[0].pipe=1;c.ep[0].data=calloc(4,FTU_BLOCK_BYTES);c.ep[0].frames=calloc(4*64,sizeof *c.ep[0].frames);
    c.ep[0].blocks[0].owner=&c;c.ep[0].blocks[0].direction=0;c.ep[0].blocks[0].index=0;
    for(unsigned i=0;i<64;i++)lengths[i]=312;
    assert(pool_submit(&c,0,0,1,300,lengths)==0);saved_cb(saved_ref,kIOReturnAborted,saved_frames);
    assert(pool_completion(&c,0,0,&completion)==1 && completion.result.status==kIOReturnAborted);
    assert(pool_submit(&c,0,0,2,308,lengths)==0);saved_cb(saved_ref,0,NULL);
    assert(pool_completion(&c,0,0,&completion)==1 && !completion.result.matched);
    assert(pool_submit(&c,0,0,3,316,lengths)==kIOReturnBusy);
    free(c.ep[0].data);free(c.ep[0].frames);
    pool_context=NULL;free(c.ep[1].data);free(c.ep[1].frames);
    three_slot_native_boundary();
    callback_pump_link();
    puts("native reusable callback identity: passed");return 0;
}
