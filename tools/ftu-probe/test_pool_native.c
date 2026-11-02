/* No hardware: test the actual callback/token adapter with fake IOUSBLib submits. */
#include "duplex.c"
#include <assert.h>
static unsigned expected_entries=64;
static unsigned allocation_calls, allocation_fail, freed_buffers;
static size_t allocated_bytes[2];
static UInt32 allocated_types[2];
static void *saved_buffer;
static IOReturn submit_status;
static IOAsyncCallback1 saved_cb;
static void *saved_ref;
static IOUSBLowLatencyIsocFrame *saved_frames;
static IOReturn fake_submit(void *self,UInt8 pipe,void *buf,UInt64 start,UInt32 count,UInt32 frequency,IOUSBLowLatencyIsocFrame *frames,IOAsyncCallback1 callback,void *ref)
{
    (void)self;(void)pipe;(void)start;
    assert(count==expected_entries && frequency==0);saved_buffer=buf;saved_cb=callback;saved_ref=ref;saved_frames=frames;return submit_status;
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
struct dispatch_pair {void *refs[2];IOUSBLowLatencyIsocFrame *frames[2];bool duplicate;};
static void dispatch_pair(void *info) {
    struct dispatch_pair *p=info;
    for(unsigned i=0;i<2;i++)pool_completed(p->refs[i],0,p->frames[i]);
    if(p->duplicate)pool_completed(p->refs[0],0,p->frames[0]);
}
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
    struct ftu_pump_observation observation;last_pump(&c,&observation);
    assert(observation.callbacks==1 && observation.first_callback_ticks==b->callback_ticks && observation.last_callback_ticks==b->callback_ticks && !observation.invalid_callbacks);
    struct ftu_pump original=b->pump;
    pump(&c);assert(c.pump_id==2);last_pump(&c,&observation);
    assert(observation.pump.id==2 && observation.pump.result==kCFRunLoopRunTimedOut && !observation.callbacks && !observation.first_callback_ticks && !observation.last_callback_ticks);
    struct ftu_pool_completion done={0};assert(pool_completion(&c,1,0,&done)==1);
    assert(memcmp(&done.pump,&original,sizeof original)==0);
    assert(pool_submit(&c,1,0,2,108,lengths)==0);
    /* Out-of-pump completion cannot inherit an earlier generation's link. */
    saved_cb(saved_ref,0,saved_frames);assert(pool_completion(&c,1,0,&done)==1);
    assert(done.pump.id==0 && done.pump.exit_ticks==0);
    /* Two independent callbacks in one source dispatch; duplicate is visible
     * without permitting slot reuse or corrupting the original completion. */
    struct dispatch_pair pair={0};
    for(unsigned i=0;i<2;i++){
        struct block *slot=&c.ep[1].blocks[i];slot->owner=&c;slot->direction=1;slot->index=i;
        assert(pool_submit(&c,1,i,10+i,200+8*i,lengths)==0);
        pair.refs[i]=saved_ref;pair.frames[i]=saved_frames;
    }
    CFRunLoopSourceContext pc={0};pc.perform=dispatch_pair;pc.info=&pair;
    CFRunLoopSourceRef both=CFRunLoopSourceCreate(NULL,0,&pc);assert(both);
    CFRunLoopAddSource(CFRunLoopGetCurrent(),both,kCFRunLoopDefaultMode);
    CFRunLoopSourceSignal(both);pump(&c);last_pump(&c,&observation);
    assert(observation.callbacks==2 && !observation.invalid_callbacks && observation.first_callback_ticks<=observation.last_callback_ticks);
    for(unsigned i=0;i<2;i++)assert(pool_completion(&c,1,i,&done)==1 && done.ticket==10+i);
    for(unsigned i=0;i<2;i++){
        assert(pool_submit(&c,1,i,20+i,220+8*i,lengths)==0);pair.refs[i]=saved_ref;pair.frames[i]=saved_frames;
    }
    pair.duplicate=true;CFRunLoopSourceSignal(both);pump(&c);last_pump(&c,&observation);
    assert(observation.callbacks==3 && observation.invalid_callbacks==1 && c.report.callback_fault);
    for(unsigned i=0;i<2;i++)assert(pool_completion(&c,1,i,&done)==1 && done.ticket==20+i);
    c.report.callback_fault=0;
    CFRunLoopRemoveSource(CFRunLoopGetCurrent(),both,kCFRunLoopDefaultMode);CFRelease(both);
    c.pump_id=UINT64_MAX-1;
    assert(pool_submit(&c,1,0,3,116,lengths)==0);CFRunLoopSourceSignal(source);pump(&c);
    assert(pool_completion(&c,1,0,&done)==1 && done.pump.id==0);
    last_pump(&c,&observation);assert(!observation.pump.id && observation.callbacks==1);
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

static unsigned restore_calls, close_calls, release_calls;
static IOReturn fake_restore(void *self,UInt8 alt) {(void)self;assert(alt==0);restore_calls++;return 0;}
static IOReturn fake_get_alt(void *self,UInt8 *alt) {(void)self;*alt=0;return 0;}
static IOReturn fake_close_interface(void *self) {(void)self;close_calls++;return 0;}
static ULONG fake_release_interface(void *self) {(void)self;release_calls++;return 0;}
static unsigned rate_requests;
static UInt32 rate_actual;
static IOReturn rate_status;
static UInt16 rate_endpoint;
static IOReturn fake_rate_request(void *self,IOUSBDevRequestTO *q)
{
    assert(self);
    assert(q->bmRequestType==0x22 && q->bRequest==1 && q->wValue==0x100);
    assert(q->wIndex==rate_endpoint && q->wLength==3);
    assert(q->noDataTimeout==500 && q->completionTimeout==500);
    const uint8_t expected[3]={0x80,0xbb,0};
    assert(memcmp(q->pData,expected,3)==0);
    rate_requests++;
    q->wLenDone=rate_actual;
    return rate_status;
}
static void startup_rate_boundary(void)
{
    struct native c={0};c.count=4;c.pool_mode=true;pool_context=&c;
    IOUSBDeviceInterface500 dev={0};dev.DeviceRequestTO=fake_rate_request;
    IOUSBDeviceInterface500 *dp=&dev;c.dev=&dp;
    IOUSBInterfaceInterface300 intf={0};IOUSBInterfaceInterface300 *ip=&intf;
    intf.SetAlternateInterface=fake_restore;intf.GetAlternateSetting=fake_get_alt;
    intf.USBInterfaceClose=fake_close_interface;intf.Release=fake_release_interface;
    for(unsigned d=0;d<2;d++){
        c.ep[d].intf=&ip;c.ep[d].opened=true;c.ep[d].alt_attempted=true;c.ep[d].pipe=1;
    }
    uint32_t actual=0;
    for(unsigned d=0;d<2;d++){
        rate_endpoint=d?0x01:0x81;
        rate_status=0;rate_actual=3;
        assert(live_set_rate(&c,d,48000,&actual)==0 && actual==3);
        rate_actual=2;assert(live_set_rate(&c,d,48000,&actual)==0 && actual==2);
        rate_status=kIOReturnTimeout;rate_actual=UINT32_MAX;
        assert(live_set_rate(&c,d,48000,&actual)==kIOReturnTimeout && actual==UINT32_MAX);
    }
    unsigned before=rate_requests;
    assert(live_set_rate(&c,0,96000,&actual)==kIOReturnBadArgument && actual==UINT32_MAX);
    assert(live_set_rate(&c,2,48000,&actual)==kIOReturnBadArgument);
    assert(live_set_rate(&c,0,48000,NULL)==kIOReturnBadArgument);
    assert(live_set_rate(NULL,0,48000,&actual)==kIOReturnBadArgument);
    for(unsigned d=0;d<2;d++){
        c.ep[d].alt_attempted=false;
        assert(live_set_rate(&c,0,48000,&actual)==kIOReturnNotOpen);
        c.ep[d].alt_attempted=true;
        c.ep[d].blocks[3].used=true;
        assert(live_set_rate(&c,0,48000,&actual)==kIOReturnBusy);
        c.ep[d].blocks[3].used=false;
    }
    pool_context=NULL;
    assert(live_set_rate(&c,0,48000,&actual)==kIOReturnBadArgument);
    assert(rate_requests==before);
    /* A failed startup control request still restores both original alts and closes. */
    pool_context=&c;c.report.open_mask=3;
    for(unsigned d=0;d<2;d++)c.ep[d].original_known=true;
    rate_endpoint=0x81;rate_status=kIOReturnTimeout;rate_actual=0;
    assert(live_set_rate(&c,0,48000,&actual)==kIOReturnTimeout);
    assert(cleanup(&c)==0 && c.report.restored_mask==3 && c.report.open_mask==0);
    assert(restore_calls==2 && close_calls==2 && release_calls==2);
    assert(!c.ep[0].intf && !c.ep[1].intf);
    pool_context=NULL;
}


/* Exact allocator extents under ASan catch a stale 64-entry stride even when
 * the public completion keeps a 64-entry capacity for ABI stability. */
static IOReturn fake_allocate(void *self,void **buffer,IOByteCount bytes,UInt32 type)
{
    (void)self;unsigned call=allocation_calls++;
    assert(call<2);allocated_bytes[call]=bytes;allocated_types[call]=type;
    if(allocation_fail==call+1)return kIOReturnNoMemory;
    *buffer=malloc(bytes);assert(*buffer);memset(*buffer,0xa5,bytes);return 0;
}
static IOReturn fake_destroy(void *self,void *buffer) {(void)self;free(buffer);freed_buffers++;return 0;}
static ULONG geometry_release(void *self) {(void)self;return 0;}
static void configurable_native_boundary(void)
{
    const unsigned profiles[][3]={{64,4,32},{32,8,64},{32,10,64},{16,16,128},{8,32,256}};
    for(unsigned profile=0;profile<sizeof profiles/sizeof profiles[0];profile++){
        unsigned entries=profiles[profile][0],slots=profiles[profile][1],history=profiles[profile][2];
        struct ftu_transport_geometry g={1,entries,slots,history,64,3072};assert(valid_geometry(&g));
        struct native c={0};c.count=slots;c.entries=entries;c.pool_mode=true;pool_context=&c;
        IOUSBInterfaceInterface300 table={0};IOUSBInterfaceInterface300 *ptr=&table;
        table.LowLatencyCreateBuffer=fake_allocate;table.LowLatencyDestroyBuffer=fake_destroy;table.Release=geometry_release;
        table.LowLatencyWriteIsochPipeAsync=fake_submit;table.LowLatencyReadIsochPipeAsync=fake_submit;
        expected_entries=entries;allocation_fail=0;submit_status=0;
        for(unsigned d=0;d<2;d++){
            c.ep[d].intf=&ptr;c.ep[d].pipe=1;allocation_calls=0;
            assert(allocate_buffers(&c,d)==0 && allocation_calls==2);
            assert(allocated_bytes[0]==slots*entries*312 && allocated_bytes[1]==slots*entries*sizeof *c.ep[d].frames);
            assert(allocated_types[0]==(d?kUSBLowLatencyWriteBuffer:kUSBLowLatencyReadBuffer) && allocated_types[1]==kUSBLowLatencyFrameListBuffer);
            for(unsigned lap=0;lap<4;lap++)for(unsigned slot=0;slot<slots;slot++){
                uint16_t lengths[64]={0};for(unsigned i=0;i<entries;i++)lengths[i]=d?144:312;
                uint64_t ticket=1+lap*slots+slot;
                assert(pool_submit(&c,d,slot,ticket,100+ticket*(entries/8),lengths)==0);
                assert(saved_buffer==c.ep[d].data+slot*entries*312 && saved_frames==c.ep[d].frames+slot*entries);
                assert(buffer(&c,d,slot)==NULL && cleanup(&c)==FTU_PENDING_OWNERSHIP);
                assert(pool_submit(&c,d,slot,ticket+1,200,lengths)==kIOReturnBusy);
                void *token=saved_ref;IOUSBLowLatencyIsocFrame *frames=saved_frames;
                for(unsigned i=0;i<entries;i++){frames[i].frStatus=0;frames[i].frActCount=d?144:120;}
                saved_cb(token,0,frames);
                struct ftu_pool_completion done;memset(&done,0xa5,sizeof done);
                assert(pool_completion(&c,d,slot,&done)==1 && done.ticket==ticket && done.packet_count==entries && done.reserved==0);
                for(unsigned i=0;i<entries;i++)assert(done.result.packets[i].actual==(d?144:120));
                struct ftu_packet zero={0};for(unsigned i=entries;i<64;i++)assert(!memcmp(&done.result.packets[i],&zero,sizeof zero));
                assert(pool_completion(&c,d,slot,&done)==0 && buffer(&c,d,slot)==saved_buffer);
                /* A duplicate cannot alter consumed ownership. */
                saved_cb(token,0,frames);assert(c.report.callback_fault);c.report.callback_fault=0;
                if(lap==3 && slot==slots-1){
                    assert(pool_submit(&c,d,slot,ticket+1,300,lengths)==0);
                    saved_cb(token,0,frames);assert(c.report.callback_fault && c.ep[d].blocks[slot].pending);
                    c.report.callback_fault=0;saved_cb(saved_ref,kIOReturnAborted,saved_frames);
                    assert(pool_completion(&c,d,slot,&done)==1 && done.result.status==kIOReturnAborted);
                    submit_status=kIOReturnNoResources;assert(pool_submit(&c,d,slot,ticket+2,400,lengths)==kIOReturnNoResources);
                    assert(!c.ep[d].blocks[slot].pending);submit_status=0;
                }
            }
        }
        freed_buffers=0;assert(cleanup(&c)==0 && freed_buffers==4);pool_context=NULL;
        /* Failed second allocation leaves only the first buffer for cleanup. */
        memset(&c,0,sizeof c);c.count=slots;c.entries=entries;c.ep[0].intf=&ptr;
        allocation_calls=0;allocation_fail=2;freed_buffers=0;
        assert(allocate_buffers(&c,0)==kIOReturnNoMemory && c.ep[0].data && !c.ep[0].frames);
        assert(cleanup(&c)==0 && freed_buffers==1);allocation_fail=0;
        /* Invalid requests are rejected before device discovery or allocation. */
        for(unsigned field=0;field<6;field++){
            struct ftu_transport_geometry invalid=g;
            switch(field){case 0:invalid.version++;break;case 1:invalid.entries--;break;case 2:invalid.slots=33;break;case 3:invalid.history/=2;break;case 4:invalid.replay_ms++;break;default:invalid.pcm_offset++;}
            memset(&c,0,sizeof c);c.pool_mode=true;pool_context=&c;allocation_calls=0;
            assert(pool_prepare(&c,&invalid)==kIOReturnBadArgument && c.count==0 && c.entries==0 && allocation_calls==0);
        }
        assert(pool_prepare(&c,NULL)==kIOReturnBadArgument);pool_context=NULL;
    }
    expected_entries=64;
}

int main(void)
{
    configurable_native_boundary();
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
    startup_rate_boundary();
    three_slot_native_boundary();
    callback_pump_link();
    puts("native reusable callback identity: passed");return 0;
}
