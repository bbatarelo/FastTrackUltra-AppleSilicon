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
    puts("native reusable callback identity: passed");return 0;
}
