/* Actual vtable/property code, real Rust API, fake native discovery. No USB. */
#include <assert.h>
#include <math.h>
#include "../shared/ftu_live.h"
/* Scoped state-machine fixture for successful/immediate recovery ordering.
 * Outside this one test, all calls go to the real linked Rust implementation. */
static void fixture_status(struct ftu_live *, struct ftu_live_status *);
static int32_t fixture_recover(struct ftu_live *, uint64_t);
#define ftu_live_status(...) fixture_status(__VA_ARGS__)
#define ftu_live_recover(...) fixture_recover(__VA_ARGS__)
#include "../driver/plugin.c"
#undef ftu_live_status
#undef ftu_live_recover
static bool recovery_order_fixture;
static uint32_t recovery_order_phase;
static int32_t recovery_order_result;
static unsigned recovery_order_calls, recovery_order_notes, recovery_order_expected_notes;
static void fixture_status(struct ftu_live *h,struct ftu_live_status *s){
    ftu_live_status(h,s);
    if(recovery_order_fixture)s->phase=recovery_order_phase;
}
static int32_t fixture_recover(struct ftu_live *h,uint64_t binding){
    if(!recovery_order_fixture)return ftu_live_recover(h,binding);
    /* Immediate recovery precedes stop notification; a failed restart's later
     * polls may follow the one truthful stopped-engine notification. */
    assert(recovery_order_notes==recovery_order_expected_notes);
    recovery_order_calls++;
    if(recovery_order_result==0)recovery_order_phase=2;
    return recovery_order_result;
}
/* Fake discovery: classification plus product/location/USB generation. */
static int32_t fake_result=FTU_USB_NONE;
static struct ftu_usb_unit fake_unit={0x2080,123,1000};
int32_t ftu_usb_discover(struct ftu_usb_unit *u){
    memset(u,0,sizeof *u);if(fake_result==FTU_USB_ONE)*u=fake_unit;return fake_result;
}
static void set_unit(int32_t result,uint32_t product,uint32_t location,uint64_t registry){
    fake_result=result;fake_unit=(struct ftu_usb_unit){product,location,registry};
}
/* Worker-side open records the binding it was given; failing keeps it offline.
 * With quarantine_next, a non-null context without ops quarantines the session.
 * Like the native factory, a pinned open of an enumeration other than the one
 * currently present is refused (counted) before any context could exist. */
static uint32_t opened_product,opened_location;static uint64_t opened_registry;
static unsigned opens,pinned_refusals;static bool quarantine_next;
static char retained_context;
static void *unavailable(uint32_t p,uint32_t l,uint64_t r){
    opens++;opened_product=p;opened_location=l;opened_registry=r;
    if(r && (fake_result!=FTU_USB_ONE || fake_unit.registry_id!=r)){pinned_refusals++;return NULL;}
    return quarantine_next?&retained_context:NULL;
}
static const struct ftu_pool_ops *no_ops(void *p){(void)p;return NULL;}
#ifdef FTU_INDEPENDENT
/* The independent worker links the live route; these tests never attach one. */
void *ftu_live_route_attach(void *ctx,const void *hooks){(void)ctx;(void)hooks;return NULL;}
uint32_t ftu_live_route_fault(void *route){(void)route;return 0;}
void ftu_live_cleanup_snapshot(void *ctx,uint64_t *words){(void)ctx;memset(words,0,16*sizeof *words);}
#endif
static int32_t no_close(void *p){(void)p;return -1;}
static int32_t no_schedule(void *p,const struct ftu_thread_schedule *s){(void)p;(void)s;return -1;}
static int32_t no_rate(void *p,uint32_t d,uint32_t hz,uint32_t *actual){(void)p;(void)d;(void)hz;(void)actual;return -1;}
const struct ftu_live_factory ftu_usb_factory={unavailable,no_ops,no_close,no_schedule,no_rate};
static UInt64 requested;
static unsigned list_notes,alive_notes;
static OSStatus changed(AudioServerPlugInHostRef h,AudioObjectID o,UInt32 n,const AudioObjectPropertyAddress *p){
    (void)h;(void)o;
    if(recovery_order_fixture)for(UInt32 i=0;i<n;i++)recovery_order_notes+=p[i].mSelector==kAudioDevicePropertyDeviceIsRunning;
    for(UInt32 i=0;i<n;i++){list_notes+=p[i].mSelector==kAudioPlugInPropertyDeviceList;alive_notes+=p[i].mSelector==kAudioDevicePropertyDeviceIsAlive;}
    return 0;
}
static OSStatus request(AudioServerPlugInHostRef h,AudioObjectID o,UInt64 action,void *p){(void)h;(void)o;(void)p;requested=action;return 0;}
static UInt32 get32(AudioObjectID obj,UInt32 selector,UInt32 scope){
    AudioObjectPropertyAddress a={selector,scope,0};UInt32 size=0,value=0;
    assert(ftu_get_size(g_ref,obj,0,&a,0,NULL,&size)==0 && size==4);
    assert(ftu_get_data(g_ref,obj,0,&a,0,NULL,4,&size,&value)==0 && size==4);return value;
}
static void owned_objects_qualifiers(void){
    AudioObjectPropertyAddress a={kAudioObjectPropertyOwnedObjects,kAudioObjectPropertyScopeGlobal,0};
    UInt32 size=99;AudioObjectID ids[2]={99,99};
    AudioClassID classes[]={kAudioLevelControlClassID,kAudioStreamClassID};
    assert(ftu_get_size(g_ref,2,0,&a,sizeof classes[0],classes,&size)==0 && size==0);
    assert(ftu_get_data(g_ref,2,0,&a,sizeof classes[0],classes,sizeof ids,&size,ids)==0 && size==0 && ids[0]==99);
    assert(ftu_get_size(g_ref,2,0,&a,sizeof classes,classes,&size)==0 && size==8);
    assert(ftu_get_data(g_ref,2,0,&a,sizeof classes,classes,sizeof ids,&size,ids)==0 && size==8 && ids[0]==3 && ids[1]==4);
    classes[0]=kAudioObjectClassID;
    assert(ftu_get_size(g_ref,2,0,&a,sizeof classes[0],classes,&size)==0 && size==8);
    assert(ftu_get_size(g_ref,1,0,&a,sizeof classes[0],classes,&size)==0 && size==4);
    classes[0]=kAudioDeviceClassID;
    assert(ftu_get_size(g_ref,1,0,&a,sizeof classes[0],classes,&size)==0 && size==4);
    assert(ftu_get_size(g_ref,2,0,&a,sizeof classes[0],classes,&size)==0 && size==0);
    classes[0]=kAudioStreamClassID;
    assert(ftu_get_size(g_ref,1,0,&a,sizeof classes[0],classes,&size)==0 && size==0);
    assert(ftu_get_data(g_ref,2,0,&a,sizeof classes[0],classes,sizeof ids[0],&size,ids)==0 && size==4 && ids[0]==3);
    a.mScope=kAudioObjectPropertyScopeOutput;
    uint8_t unaligned[sizeof(AudioClassID)+1];memcpy(unaligned+1,classes,sizeof classes[0]);
    assert(ftu_get_data(g_ref,2,0,&a,sizeof classes[0],unaligned+1,sizeof ids,&size,ids)==0 && size==4 && ids[0]==4);
    assert(ftu_get_size(g_ref,2,0,&a,sizeof(AudioClassID)-1,classes,&size)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_size(g_ref,2,0,&a,sizeof(AudioClassID),NULL,&size)==kAudioHardwareBadPropertySizeError);
}
static void expect_present(bool present,bool alive){
    AudioObjectPropertyAddress a={kAudioPlugInPropertyDeviceList,kAudioObjectPropertyScopeGlobal,0};UInt32 size=99;
    assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==(present?4u:0u));
    assert(get32(2,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal)==(alive?1u:0u));
}
static void hotplug_json(char *text,size_t n){
    AudioObjectPropertyAddress a={FTU_HOTPLUG_PROPERTY,kAudioObjectPropertyScopeGlobal,0};CFStringRef s=NULL;UInt32 size=0;
    assert(ftu_has_property(g_ref,2,0,&a));
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof s,&size,&s)==0 && s && size==sizeof s);
    assert(CFStringGetCString(s,text,(CFIndex)n,kCFStringEncodingUTF8));CFRelease(s);
    if(getenv("FTU_TEST_HOTPLUG_JSON"))puts(text);
}
static uint64_t session_epoch(void){struct ftu_live_status st;memset(&st,0,sizeof st);ftu_live_status(live_handle(),&st);return st.epoch;}
/* StartIO reaches the worker, which opens exactly the bound location and USB
 * enumeration (always pinned in the HAL) in a fresh epoch. */
static void start_opens(uint32_t location){
    unsigned before=opens,refused=pinned_refusals;uint64_t epoch=session_epoch();
    assert(ftu_start_io(g_ref,2,1)==kAudioHardwareUnspecifiedError);
    assert(opens==before+1 && opened_product==0x2080 && opened_location==location);
    assert(opened_registry && opened_registry==g_unit.registry_id && pinned_refusals==refused);
    assert(session_epoch()==epoch+1);
    struct ftu_live_status st;memset(&st,0,sizeof st);ftu_live_status(live_handle(),&st);assert(st.location==location);
    assert(ftu_stop_io(g_ref,2,1)==0);
}
/* Removal is published in one poll and arrival only on a later one. */
static void reconnect(uint32_t location,uint64_t registry){
    unsigned list=list_notes,alive=alive_notes;
    set_unit(FTU_USB_ONE,0x2080,location,registry);
    poll_presence(true);expect_present(false,false);
    assert(list_notes==list+1 && alive_notes==alive+1);
    poll_presence(true);expect_present(true,true);
    assert(list_notes==list+2 && alive_notes==alive+2);
    poll_presence(true);expect_present(true,true);assert(list_notes==list+2);
}
static void hotplug_regressions(void){
    char json[1024];struct ftu_live *resident=live_handle();
    AudioObjectPropertyAddress a={kAudioDevicePropertyDeviceUID,kAudioObjectPropertyScopeGlobal,0};CFStringRef uid=NULL;UInt32 size=0;
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof uid,&size,&uid)==0 && uid);
    assert(CFEqual(uid,CFSTR("net.batarelo.FastTrackUltra:2080:single")));
    UInt32 id=0;
    a=(AudioObjectPropertyAddress){kAudioObjectPropertyCustomPropertyInfoList,kAudioObjectPropertyScopeGlobal,0};
    AudioServerPlugInCustomPropertyInfo props[10];
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof props,&size,props)==0 && size==sizeof props);
    assert(props[9].mSelector==FTU_HOTPLUG_PROPERTY);
    /* Unchanged unit and generation: steady, no notifications, no open. */
    set_unit(FTU_USB_ONE,0x2080,123,1000);
    unsigned list=list_notes,alive=alive_notes,before=opens;
    poll_presence(true);expect_present(true,true);assert(list_notes==list && alive_notes==alive && opens==before);
    /* Absence => removal. No start reaches USB while detached; polling stays inert. */
    set_unit(FTU_USB_NONE,0,0,0);poll_presence(true);expect_present(false,false);
    assert(list_notes==list+1 && alive_notes==alive+1);
    assert(ftu_start_io(g_ref,2,1)==kAudioHardwareNotRunningError);
    assert(ftu_live_start(live_handle(),9)==-6 && opens==before);
    for(unsigned i=0;i<4;i++)poll_presence(true);
    expect_present(false,false);assert(list_notes==list+1 && opens==before);
    /* A delayed StopIO for a client of the removed unit is still answered. */
    assert(ftu_stop_io(g_ref,2,77)==kAudioHardwareNoError);
    a=(AudioObjectPropertyAddress){kAudioPlugInPropertyTranslateUIDToDevice,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_get_data(g_ref,1,0,&a,sizeof uid,&uid,sizeof id,&size,&id)==0 && id==kAudioObjectUnknown);
    /* Arrival at the same location, new generation => rebind on a later poll. */
    set_unit(FTU_USB_ONE,0x2080,123,1001);poll_presence(true);expect_present(true,true);
    assert(list_notes==list+2 && alive_notes==alive+2);
    assert(ftu_get_data(g_ref,1,0,&a,sizeof uid,&uid,sizeof id,&size,&id)==0 && id==kObjDevice);
    start_opens(123);
    /* Replacement enumeration at the same location before any poll sees it: the
     * worker stays pinned to 1001 and cannot open 1009 without detach/rebind. */
    unsigned pinned_opens=opens,refused=pinned_refusals;uint64_t old_binding=ftu_live_binding(live_handle());
    set_unit(FTU_USB_ONE,0x2080,123,1009);
    assert(ftu_start_io(g_ref,2,1)==kAudioHardwareUnspecifiedError);
    assert(opens==pinned_opens+1 && opened_registry==1001 && pinned_refusals==refused+1);
    assert(ftu_stop_io(g_ref,2,1)==0);
    poll_presence(true);expect_present(false,false);
    poll_presence(true);expect_present(true,true);
    /* A Start that read the binding before the rebind is refused and never opens 1009. */
    assert(ftu_live_start_bound(live_handle(),1,old_binding)==-6 && opens==pinned_opens+1);
    start_opens(123);assert(opened_registry==1009);
    /* Observed Oct 26: direct port, then hub 0x20211100, never seen absent. */
    reconnect(0x02100000,1002);start_opens(0x02100000);
    reconnect(0x20211100,1003);start_opens(0x20211100);
    hotplug_json(json,sizeof json);
    assert(strstr(json,"\"coherent\":1") && strstr(json,"\"bound\":1") && strstr(json,"\"location\":539037952"));
    assert(strstr(json,"\"registry_id\":1003") && strstr(json,"\"rebinds\":4") && strstr(json,"\"removals\":4"));
    assert(strstr(json,"\"detach_status\":0") && strstr(json,"\"rebind_status\":0"));
    /* Same port, new USB generation between polls (fast unplug/replug). */
    reconnect(0x20211100,1004);start_opens(0x20211100);
    /* Repeated reconnects: one resident handle, fresh epochs, bounded work. */
    for(unsigned i=0;i<50;i++){
        set_unit(FTU_USB_NONE,0,0,0);poll_presence(true);expect_present(false,false);
        set_unit(FTU_USB_ONE,0x2080,0x100+i%3,2000+i);poll_presence(true);expect_present(true,true);
        start_opens(0x100+i%3);assert(live_handle()==resident);
    }
    /* Ambiguous discovery: a second supported unit detaches; nothing is chosen. */
    set_unit(FTU_USB_AMBIGUOUS,0,0,0);poll_presence(true);expect_present(false,false);
    before=opens;poll_presence(true);expect_present(false,false);
    assert(ftu_live_start(live_handle(),9)==-6 && opens==before);
    hotplug_json(json,sizeof json);assert(strstr(json,"\"bound\":0") && strstr(json,"\"reject\":1"));
    set_unit(FTU_USB_ONE,0x2080,0x300,3000);poll_presence(true);expect_present(true,true);
    /* Model change: an 8R replacing the adopted Ultra is refused, never attached. */
    set_unit(FTU_USB_ONE,0x2081,0x300,3001);poll_presence(true);expect_present(false,false);
    poll_presence(true);expect_present(false,false);
    hotplug_json(json,sizeof json);assert(strstr(json,"\"reject\":2") && strstr(json,"\"product\":8320"));
    assert(ftu_live_rebind(live_handle(),0x2081,0x300,3001)==-7);
    a=(AudioObjectPropertyAddress){kAudioDevicePropertyDeviceUID,kAudioObjectPropertyScopeGlobal,0};CFStringRef again=NULL;
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof again,&size,&again)==0 && again==uid);CFRelease(again);CFRelease(uid);
    set_unit(FTU_USB_ONE,0x2080,0x300,3002);poll_presence(true);expect_present(true,true);
    /* Rebinding requires a detach first. */
    assert(ftu_live_rebind(live_handle(),0x2080,0x300,3002)==-8);
    /* A registry scan that raced enumeration carries no information: no flapping. */
    list=list_notes;set_unit(FTU_USB_UNKNOWN,0,0,0);poll_presence(true);expect_present(true,true);
    set_unit(FTU_USB_ONE,0x2080,0x300,3002);poll_presence(true);expect_present(true,true);assert(list_notes==list);
    start_opens(0x300);
}
static void property_json(UInt32 selector,char *text,size_t n){
    AudioObjectPropertyAddress a={selector,kAudioObjectPropertyScopeGlobal,0};CFStringRef s=NULL;UInt32 size=0,frames=128;Boolean settable=true;
    assert(ftu_has_property(g_ref,2,0,&a));
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_size(g_ref,2,0,&a,0,NULL,&size)==0 && size==sizeof s);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof s-1,&size,&s)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof s,&size,&s)==0 && s && size==sizeof s);
    assert(CFStringGetCString(s,text,(CFIndex)n,kCFStringEncodingUTF8));CFRelease(s);
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
}
static void text_of(CFStringRef s,char *text,size_t n){assert(s && CFStringGetCString(s,text,(CFIndex)n,kCFStringEncodingUTF8));CFRelease(s);}
/* ftuE/ftuA: ABI, property list, read-only shape and null-vs-zero formatting. */
/* Actual plug-in monitor ordering, with just phase/recovery responses injected.
 * Full reopen/clock/client ownership is separately tested by the linked worker. */
static void monitor_recovers_before_announcing_a_transient_stop(void){
    assert(g_bound && live_handle());
    recovery_order_fixture=true;recovery_order_calls=0;recovery_order_notes=0;
    recovery_order_phase=4;recovery_order_result=0;atomic_store(&g_running,true);
    poll_presence(true);
    assert(recovery_order_calls==1 && recovery_order_notes==0);
    assert(atomic_load(&g_running) && get32(2,kAudioDevicePropertyDeviceIsRunning,kAudioObjectPropertyScopeGlobal)==1);
    /* Running polls keep policy health counters current. */
    recovery_order_result=1;poll_presence(true);
    assert(recovery_order_calls==2 && recovery_order_notes==0);
    /* A just-finished worker or a concurrent control call gets a bounded
     * notification grace; actual DeviceIsRunning still reports the fault. */
    for(int32_t wait=3;wait<=4;wait++){
        recovery_order_notes=0;recovery_order_phase=4;recovery_order_result=wait;
        atomic_store(&g_running,true);
        for(unsigned i=0;i<FTU_RECOVERY_WAIT_POLLS;i++){
            poll_presence(true);
            assert(recovery_order_notes==0 && atomic_load(&g_running));
            assert(get32(2,kAudioDevicePropertyDeviceIsRunning,kAudioObjectPropertyScopeGlobal)==0);
        }
        recovery_order_result=0;poll_presence(true);
        assert(recovery_order_notes==0 && atomic_load(&g_running));
        /* Persistent contention cannot conceal a stopped engine indefinitely. */
        recovery_order_phase=4;recovery_order_result=wait;
        for(unsigned i=0;i<=FTU_RECOVERY_WAIT_POLLS;i++)poll_presence(true);
        assert(recovery_order_notes==1 && !atomic_load(&g_running));
    }
    /* Exhaustion/refusal/backoff stay visibly stopped, with one notification. */
    for(unsigned i=0;i<3;i++){
        recovery_order_notes=0;recovery_order_phase=4;
        recovery_order_result=i==0?-11:i==1?-10:2;atomic_store(&g_running,true);
        poll_presence(true);
        assert(recovery_order_notes==1 && !atomic_load(&g_running));
    }
    /* A clean failed reopen returns -5 while Rust schedules another attempt.
     * The real monitor must keep polling while g_running is already false:
     * wait, then recover without needing a fresh client Start. */
    unsigned before=recovery_order_calls;
    recovery_order_notes=0;recovery_order_phase=4;recovery_order_result=-5;
    atomic_store(&g_running,true);
    poll_presence(true);
    assert(recovery_order_calls==before+1 && recovery_order_notes==1 && !atomic_load(&g_running));
    assert(atomic_load(&g_recover_status)==-5 && g_recover_wait_polls==0);
    recovery_order_expected_notes=1;recovery_order_result=2;
    for(unsigned i=0;i<4;i++){
        poll_presence(true);
        assert(recovery_order_calls==before+2+i && recovery_order_notes==1 && !atomic_load(&g_running));
        assert(atomic_load(&g_recover_status)==2);
    }
    recovery_order_result=0;poll_presence(true);
    assert(recovery_order_calls==before+6 && recovery_order_notes==2 && atomic_load(&g_running));
    assert(atomic_load(&g_recover_status)==0);
    recovery_order_expected_notes=0;
    recovery_order_fixture=false;atomic_store(&g_running,false);
}

static void runtime_recovery_properties(void){
    _Static_assert(sizeof(struct ftu_runtime_snapshot)==368,"runtime fault ABI");
    _Static_assert(sizeof(struct ftu_recovery_snapshot)==240,"recovery ABI");
    char json[8192];UInt32 size=0;
    AudioObjectPropertyAddress a={kAudioObjectPropertyCustomPropertyInfoList,kAudioObjectPropertyScopeGlobal,0};
    AudioServerPlugInCustomPropertyInfo props[FTU_CUSTOM_PROPERTIES];
    assert(ftu_get_size(g_ref,2,0,&a,0,NULL,&size)==0 && size==sizeof props);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof props,&size,props)==0 && size==sizeof props);
    assert(props[10].mSelector==FTU_RUNTIME_PROPERTY && props[11].mSelector==FTU_RECOVERY_PROPERTY);
    assert(props[10].mPropertyDataType==kAudioServerPlugInCustomPropertyDataTypeCFString);
    property_json(FTU_RUNTIME_PROPERTY,json,sizeof json);
    if(getenv("FTU_TEST_RUNTIME_JSON"))puts(json);
    assert(strstr(json,"\"version\":1,\"valid\":1,\"present\":0") && strstr(json,"\"epoch\":null"));
    assert(strstr(json,"\"checked_frame\":null") && strstr(json,"\"stage_name\":\"none\"") && strstr(json,"\"overwritten\":0"));
    property_json(FTU_RECOVERY_PROPERTY,json,sizeof json);
    if(getenv("FTU_TEST_RECOVERY_JSON"))puts(json);
#ifdef FTU_INDEPENDENT
    assert(strstr(json,"\"enabled\":1"));
#else
    assert(strstr(json,"\"enabled\":0"));
#endif
    assert(strstr(json,"\"budget\":3") && strstr(json,"\"backoff1_ms\":1000") && strstr(json,"\"healthy_ms\":30000"));
    assert(strstr(json,"\"due_ms\":null") && strstr(json,"\"monitor_calls\":"));
    /* Synthetic epoch-48 style record: OUT after_packing, no return clock. */
    struct ftu_runtime_snapshot r={0};
    r.words[0]=1;r.words[1]=1;r.words[2]=1;r.words[3]=48;r.words[4]=55;r.words[5]=5;
    r.words[6]=1|2|4|8|32|1024|2048;r.words[7]=1;r.words[8]=43734;r.words[9]=3;
    r.words[10]=180;r.words[11]=170;r.words[12]=4080000;r.words[13]=999;r.words[15]=16;
    r.words[30]=125;r.words[31]=3;r.words[33]=(uint64_t)(int64_t)-1;
    text_of(runtime_snapshot_text(&r),json,sizeof json);
    if(getenv("FTU_TEST_RUNTIME_JSON"))puts(json);
    assert(strstr(json,"\"stage_name\":\"after_packing\"") && strstr(json,"\"scheduled_frame\":180") && strstr(json,"\"checked_ticks\":4080000"));
    assert(strstr(json,"\"returned_frame\":null") && strstr(json,"\"returned_ticks\":null") && strstr(json,"\"submit_status\":null"));
    assert(strstr(json,"\"prior_progress_frame\":null") && strstr(json,"\"min_out_return_lead\":null") && strstr(json,"\"finished\":0"));
    assert(strstr(json,"\"cleanup_status\":null") && strstr(json,"\"close_status\":null") && strstr(json,"\"terminal_phase\":null"));
    r.words[35]=1;r.words[37]=0;r.words[38]=(uint64_t)(int64_t)-5;r.words[6]|=512;r.words[39]=4;r.words[40]=6;
    text_of(runtime_snapshot_text(&r),json,sizeof json);
    assert(strstr(json,"\"close_status\":-5") && strstr(json,"\"cleanup_status\":0") && strstr(json,"\"terminal_fault\":6"));
    memset(&r,0xff,sizeof r);
    text_of(runtime_snapshot_text(&r),json,sizeof json);
    assert(strstr(json,"18446744073709551615") && strstr(json,"\"stage_name\":\"unknown\"") && strstr(json,"\"submit_status\":-1"));
    struct ftu_recovery_snapshot v={0};
    v.words[0]=v.words[1]=v.words[2]=1;v.words[3]=1;v.words[16]=(uint64_t)(int64_t)-5;v.words[17]=10;v.words[18]=1010;v.words[29]=1;
    text_of(recovery_snapshot_text(&v),json,sizeof json);
    if(getenv("FTU_TEST_RECOVERY_JSON"))puts(json);
    assert(strstr(json,"\"state_name\":\"pending\"") && strstr(json,"\"due_ms\":1010") && strstr(json,"\"running_since_ms\":null") && strstr(json,"\"last_status\":-5"));
    memset(&v,0xff,sizeof v);
    text_of(recovery_snapshot_text(&v),json,sizeof json);
    assert(strstr(json,"\"state_name\":\"unknown\"") && strstr(json,"\"refusal_name\":\"unknown\"") && strstr(json,"\"last_status\":-1"));
}
/* The monitor asks Rust to recover only for the bound unit it just confirmed;
 * removal or a replacement enumeration never reaches recovery. */
static void monitor_recovery_gating(void){
    char json[8192];
    set_unit(FTU_USB_ONE,0x2080,0x500,5000);poll_presence(true);poll_presence(true);expect_present(true,true);
    start_opens(0x500); /* offline open: Fault with no admitted client */
    struct ftu_live_status st;memset(&st,0,sizeof st);ftu_live_status(live_handle(),&st);assert(st.phase==4);
    uint64_t calls=atomic_load(&g_recover_calls);unsigned before=opens;
    poll_presence(true);
    assert(atomic_load(&g_recover_calls)==calls+1 && atomic_load(&g_recover_status)==1 && opens==before);
    property_json(FTU_RECOVERY_PROPERTY,json,sizeof json);
    assert(strstr(json,"\"clients\":0") && strstr(json,"\"monitor_status\":1") && strstr(json,"\"attempts\":0"));
    /* Removal poll and unbound polls never call recovery. */
    set_unit(FTU_USB_NONE,0,0,0);poll_presence(true);poll_presence(true);
    assert(atomic_load(&g_recover_calls)==calls+1);
    assert(ftu_live_recover(live_handle(),ftu_live_binding(live_handle()))==-6);
    /* Replacement enumeration: the rebind poll is not steady; no recovery call. */
    uint64_t old=ftu_live_binding(live_handle());
    set_unit(FTU_USB_ONE,0x2080,0x500,5001);poll_presence(true);
    assert(atomic_load(&g_recover_calls)==calls+1 && ftu_live_binding(live_handle())!=old);
    assert(ftu_live_recover(live_handle(),old)==-6 && opens==before);
    poll_presence(true);assert(atomic_load(&g_recover_calls)==calls+2 && atomic_load(&g_recover_status)==1 && opens==before);
}
/* Terminal: quarantine is retained across removal and never rebound. */
static void quarantine_regression(void){
    char json[1024];
    set_unit(FTU_USB_ONE,0x2080,0x400,4000);poll_presence(true);poll_presence(true);expect_present(true,true);
    quarantine_next=true;unsigned before=opens;
    assert(ftu_start_io(g_ref,2,1)!=kAudioHardwareNoError && opens==before+1);
    quarantine_next=false;
    struct ftu_live_status st;memset(&st,0,sizeof st);ftu_live_status(live_handle(),&st);assert(st.phase==5);
    poll_presence(true);expect_present(true,false);
    set_unit(FTU_USB_NONE,0,0,0);poll_presence(true);expect_present(false,false);
    hotplug_json(json,sizeof json);assert(strstr(json,"\"detach_status\":-3"));
    unsigned arrival_alive=alive_notes;
    set_unit(FTU_USB_ONE,0x2080,0x400,4001);poll_presence(true);poll_presence(true);expect_present(true,false);
    assert(alive_notes==arrival_alive+1); /* dead arrival must invalidate host cache */
    char recovery_json[2048];property_json(FTU_RECOVERY_PROPERTY,recovery_json,sizeof recovery_json);
    assert(strstr(recovery_json,"\"state_name\":\"refused\"") && strstr(recovery_json,"\"refusal_name\":\"quarantine\""));
    hotplug_json(json,sizeof json);assert(strstr(json,"\"bound\":0") && strstr(json,"\"reject\":3"));
    assert(ftu_live_rebind(live_handle(),0x2080,0x400,4001)==-3);
    assert(ftu_start_io(g_ref,2,2)==kAudioHardwareNotRunningError && opens==before+1);
    assert(ftu_live_destroy(live_handle())!=0); /* retained, as the resident plug-in requires */
}
int main(void){
    _Static_assert(sizeof(struct ftu_live_timestamp)==24,"timestamp ABI");
    _Static_assert(sizeof(struct ftu_live_status)==152,"status ABI");
    _Static_assert(sizeof(struct ftu_timing_metric)==184,"timing metric ABI");
    _Static_assert(sizeof(struct ftu_timing_snapshot)==4576,"timing snapshot ABI");
    AudioServerPlugInHostInterface host={.PropertiesChanged=changed,.RequestDeviceConfigurationChange=request};g_host=&host;
    ftu_live_configuration(&g_cfg);atomic_store(&g_buffer_frames,g_cfg.default_buffer);
    assert(config_valid(&g_cfg));
    struct ftu_live_config invalid=g_cfg;invalid.period=512;assert(!config_valid(&invalid));
    assert(g_cfg.max_buffer==4096 && g_cfg.ring_frames>=2*g_cfg.max_buffer);
    assert(FTUFactory(NULL,kAudioServerPlugInTypeUUID)==g_ref);assert(!FTUFactory(NULL,NULL));
    AudioObjectPropertyAddress a={kAudioPlugInPropertyDeviceList,kAudioObjectPropertyScopeGlobal,0};UInt32 size=99;
    poll_presence(false);assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==0 && !live_handle());
    /* Ambiguity and transient scans never adopt a unit. */
    set_unit(FTU_USB_AMBIGUOUS,0,0,0);poll_presence(false);assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==0 && !live_handle());
    set_unit(FTU_USB_UNKNOWN,0,0,0);poll_presence(false);assert(!live_handle());
    set_unit(FTU_USB_ONE,0x2080,123,1000);poll_presence(false);assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==4);
    owned_objects_qualifiers();
    assert(get32(2,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal)==1);
    assert(get32(2,kAudioDevicePropertyZeroTimeStampPeriod,kAudioObjectPropertyScopeGlobal)>=10923);
    assert(get32(2,kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeInput)==1024);
    assert(get32(2,kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeOutput)==2560);
    assert(get32(2,kAudioDevicePropertyUsesVariableBufferFrameSizes,kAudioObjectPropertyScopeGlobal)==0);
    a=(AudioObjectPropertyAddress){kAudioStreamPropertyVirtualFormat,kAudioObjectPropertyScopeGlobal,0};AudioStreamBasicDescription f;
    assert(ftu_get_data(g_ref,4,0,&a,0,NULL,sizeof f,&size,&f)==0);
    assert(f.mSampleRate==48000 && f.mChannelsPerFrame==8 && f.mBytesPerFrame==32);
    assert(ftu_get_data(g_ref,4,0,&a,0,NULL,sizeof f-1,&size,&f)==kAudioHardwareBadPropertySizeError);
    a=(AudioObjectPropertyAddress){kAudioDevicePropertyPreferredChannelLayout,kAudioObjectPropertyScopeOutput,0};uint8_t layout[256];
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof layout,&size,layout)==0);
    assert(((AudioChannelLayout *)layout)->mNumberChannelDescriptions==8);
    a=(AudioObjectPropertyAddress){kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,0};UInt32 frames=128;
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,4,&frames)==0 && requested==128);
    assert(get32(2,a.mSelector,a.mScope)==256);assert(ftu_perform_change(g_ref,2,requested,NULL)==0);assert(get32(2,a.mSelector,a.mScope)==128);
    frames=4096;assert(ftu_set_data(g_ref,2,0,&a,0,NULL,4,&frames)==0 && requested==4096);
    assert(ftu_perform_change(g_ref,2,requested,NULL)==0 && get32(2,a.mSelector,a.mScope)==4096);
    frames=4097;assert(ftu_set_data(g_ref,2,0,&a,0,NULL,4,&frames)!=0);
    assert(ftu_start_io(g_ref,2,1)!=0);assert(ftu_stop_io(g_ref,2,1)==0);
    AudioServerPlugInIOCycleInfo cycle={0};float samples[8*4096]={0};cycle.mOutputTime.mSampleTime=NAN;
    assert(ftu_do_io(g_ref,2,4,1,kAudioServerPlugInIOOperationWriteMix,64,&cycle,samples,NULL)==kAudioHardwareIllegalOperationError);
    cycle.mOutputTime.mSampleTime=0;
    assert(ftu_do_io(g_ref,2,3,1,kAudioServerPlugInIOOperationWriteMix,64,&cycle,samples,NULL)==kAudioHardwareBadStreamError);
    assert(ftu_do_io(g_ref,2,4,1,kAudioServerPlugInIOOperationWriteMix,4097,&cycle,samples,NULL)==kAudioHardwareIllegalOperationError);
    /* A full-size request reaches the stopped engine, not the size guard. */
    assert(ftu_do_io(g_ref,2,4,1,kAudioServerPlugInIOOperationWriteMix,4096,&cycle,samples,NULL)==kAudioHardwareUnspecifiedError);
    assert(ftu_do_io(g_ref,2,3,1,kAudioServerPlugInIOOperationReadInput,4096,&cycle,samples,NULL)==kAudioHardwareUnspecifiedError);
    a=(AudioObjectPropertyAddress){FTU_STATUS_PROPERTY,kAudioObjectPropertyScopeGlobal,0};CFStringRef status=NULL;
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);CFRelease(status);
    a=(AudioObjectPropertyAddress){FTU_TIMING_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    Boolean settable=true;assert(ftu_has_property(g_ref,2,0,&a));
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    char timing_json[32768];assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));
    assert(strstr(timing_json,"\"version\":1") && strstr(timing_json,"\"steady_after\":48000"));
    assert(strstr(timing_json,"\"min\":null") && strstr(timing_json,"\"submit_return_lead_ms\""));
    CFRelease(status);
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    a=(AudioObjectPropertyAddress){kAudioObjectPropertyCustomPropertyInfoList,kAudioObjectPropertyScopeGlobal,0};
    AudioServerPlugInCustomPropertyInfo props[9];
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof props,&size,props)==0 && size==sizeof props);
    assert(props[2].mSelector==FTU_TIMING_PROPERTY && props[3].mSelector==FTU_WORKER_PROPERTY);
    _Static_assert(sizeof(struct ftu_retry_snapshot)==256,"retry ABI");
    assert(props[8].mSelector==FTU_RETRY_PROPERTY);
    a=(AudioObjectPropertyAddress){FTU_RETRY_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"version\":1") && strstr(timing_json,"\"deferrals\":0") && strstr(timing_json,"\"over_budget\":0"));
    if(getenv("FTU_TEST_RETRY_JSON"))puts(timing_json);
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_retry_snapshot retry={.header={1,1,99,1,26,16,8000000,2000000,48000,125,3,0},
        .counts={{2,2,1,1,0,1,1,0,9000000,9000000},{1,1,1,0,0,1,1,0,9000000,9000000}}};
    status=retry_snapshot_text(&retry);assert(status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"epoch\":99") && strstr(timing_json,"\"max_retry_ns\":9000000"));
    if(getenv("FTU_TEST_RETRY_JSON"))puts(timing_json);
    memset(&retry,0xff,sizeof retry);
    status=retry_snapshot_text(&retry);assert(status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"18446744073709551615"));
    _Static_assert(sizeof(struct ftu_worker_event)==256,"worker event ABI");
    _Static_assert(sizeof(struct ftu_worker_snapshot)==6792,"worker snapshot ABI");
    a=(AudioObjectPropertyAddress){FTU_WORKER_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"observed\":0") && strstr(timing_json,"\"late\":[]"));
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_worker_snapshot synthetic={.version=1,.valid=1,.epoch=77,.timebase_numer=125,.timebase_denom=3,.queue_depth=4};
    for(unsigned i=0;i<FTU_WORKER_RECENT;i++){
        for(unsigned j=0;j<FTU_WORKER_WORDS;j++)synthetic.recent[i].words[j]=UINT64_MAX;
        synthetic.recent[i].words[0]=i+1;synthetic.recent[i].words[6]=255;
    }
    synthetic.worst[0]=synthetic.recent[0];synthetic.worst[1]=synthetic.recent[1];
    for(unsigned i=0;i<FTU_WORKER_LATE;i++)synthetic.late[i]=synthetic.recent[i];
    status=worker_snapshot_text(&synthetic);assert(status);
    char *large=malloc(131072);assert(large && CFStringGetCString(status,large,131072,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(large,"\"submit_status\":-1") && strstr(large,"18446744073709551615"));free(large);
    synthetic=(struct ftu_worker_snapshot){.version=1,.valid=1};synthetic.recent[0].words[0]=1;
    status=worker_snapshot_text(&synthetic);assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"previous_sequence\":null") && strstr(timing_json,"\"returned_frame\":null"));

    _Static_assert(sizeof(struct ftu_delivery_snapshot)==6296,"delivery ABI");
    assert(props[4].mSelector==FTU_DELIVERY_PROPERTY);
    a=(AudioObjectPropertyAddress){FTU_DELIVERY_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    if(getenv("FTU_TEST_JSON"))puts(timing_json);
    assert(strstr(timing_json,"\"sample_count\":0") && strstr(timing_json,"\"min_ns\":null") && strstr(timing_json,"\"samples\":[]"));
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_delivery_snapshot delivery={0};
    memset(&delivery,0xff,sizeof delivery);delivery.header[17]=2;
    status=delivery_snapshot_text(&delivery);assert(status);
    large=malloc(131072);assert(large && CFStringGetCString(status,large,131072,kCFStringEncodingUTF8));
    assert(strstr(large,"\"cpu_status\":-1") && strstr(large,"\"cpu_user_ns\":null") && strstr(large,"\"policy_default\":null"));
    CFRelease(status);free(large);
    delivery=(struct ftu_delivery_snapshot){0};delivery.header[0]=1;delivery.header[1]=1;
    delivery.recent[0][0]=1;delivery.recent[0][14]=1;
    status=delivery_snapshot_text(&delivery);assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"pump_id\":null") && strstr(timing_json,"\"last_packet_ticks\":null"));

    _Static_assert(sizeof(struct ftu_pumps_snapshot)==11448,"all-pump snapshot ABI");
    _Static_assert(sizeof(struct ftu_pool_ops)==152,"pool ops v4 ABI");
    assert(props[5].mSelector==FTU_PUMPS_PROPERTY);
    a=(AudioObjectPropertyAddress){FTU_PUMPS_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"calls\":0") && strstr(timing_json,"\"id_discontinuities\":0"));
    if(getenv("FTU_TEST_PUMPS_JSON"))puts(timing_json);
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_pumps_snapshot pumps={0};
    pumps.header[0]=pumps.header[1]=pumps.header[3]=pumps.header[4]=pumps.header[6]=1;
    pumps.header[2]=17;pumps.header[5]=48000;
    uint64_t pc[]={1,1,0,0,0,0,1,0,0,1,0,0};memcpy(pumps.counts[1],pc,sizeof pc);
    uint64_t pr[]={1,1,48000,1,1000,2001000,3,0,0,0,0,0,0};memcpy(pumps.recent[0],pr,sizeof pr);
    for(unsigned m=0;m<3;m++){
        uint64_t *h=pumps.metrics[1][m];h[0]=1;
        if(m<2){h[1]=h[2]=2000000;h[3+12]=1;}else h[3]=1;
        memcpy(pumps.worst[1][m],pr,sizeof pr);
    }
    status=pumps_snapshot_text(&pumps);assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    if(getenv("FTU_TEST_PUMPS_JSON"))puts(timing_json);
    memset(&pumps,0xff,sizeof pumps);
    status=pumps_snapshot_text(&pumps);assert(status);
    large=malloc(131072);assert(large && CFStringGetCString(status,large,131072,kCFStringEncodingUTF8));
    assert(strstr(large,"18446744073709551615"));CFRelease(status);free(large);
    _Static_assert(sizeof(struct ftu_failure_snapshot)==3352,"failure ABI");
    assert(props[6].mSelector==FTU_FAILURE_PROPERTY);
    a=(AudioObjectPropertyAddress){FTU_FAILURE_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"present\":0") && strstr(timing_json,"\"version\":2"));
    if(getenv("FTU_TEST_FAILURE_JSON"))puts(timing_json);
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_failure_snapshot failed={0};
    failed.header[0]=failed.header[1]=failed.header[2]=failed.header[3]=1;
    failed.header[4]=23;failed.header[7]=1;
    failed.header[9]=100;failed.header[10]=failed.header[11]=84;
    failed.header[13]=0;failed.header[14]=(uint64_t)(int64_t)-536870168;
    failed.header[15]=failed.header[17]=failed.header[20]=failed.header[21]=failed.header[23]=failed.header[30]=1;
    failed.header[24]=failed.header[26]=4;
    failed.rates[0][2]=failed.rates[0][3]=48000;
    failed.rates[0][4]=failed.rates[0][5]=3;
    memcpy(failed.rates[1],failed.rates[0],sizeof failed.rates[0]);
    failed.info[0][0]=125;failed.info[0][1]=3;failed.info[0][4]=0x2080;
    failed.info[0][11]=3;memcpy(failed.info[1],failed.info[0],sizeof failed.info[0]);
    failed.info[1][10]=3;failed.info[1][11]=0;
    for(unsigned i=0;i<64;i++){failed.requested[i]=failed.packets[i][1]=312;failed.packets[i][2]=144;failed.packets[i][3]=i+1000;}
    failed.packets[0][0]=(uint64_t)(int64_t)-536870168;
    failed.packets[63][0]=0x6c6c6974;failed.packets[63][2]=0;
    status=failure_snapshot_text(&failed);assert(status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"completion_status\":-536870168"));
    if(getenv("FTU_TEST_FAILURE_JSON"))puts(timing_json);
    failed.header[0]=2;failed.header[31]=32;failed.header[32]=8;failed.header[33]=4;failed.header[34]=64;
    for(unsigned i=32;i<64;i++){failed.requested[i]=0;memset(failed.packets[i],0,sizeof failed.packets[i]);}
    failed.packets[31][0]=0x6c6c6974;failed.packets[31][2]=0;
    status=failure_snapshot_text(&failed);assert(status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"entries\":32") && strstr(timing_json,"\"queue_depth\":8"));
    if(getenv("FTU_TEST_FAILURE_JSON"))puts(timing_json);
    _Static_assert(sizeof(struct ftu_live_factory)==40,"live factory v3 ABI (pinned open)");
    _Static_assert(sizeof(struct ftu_startup_snapshot)==352,"startup ABI");
    assert(props[7].mSelector==FTU_STARTUP_PROPERTY);
    a=(AudioObjectPropertyAddress){FTU_STARTUP_PROPERTY,kAudioObjectPropertyScopeGlobal,0};
    assert(ftu_has_property(g_ref,2,0,&a));settable=true;
    assert(ftu_is_settable(g_ref,2,0,&a,&settable)==0 && !settable);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status-1,&size,&status)==kAudioHardwareBadPropertySizeError);
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof status,&size,&status)==0 && status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"rates\":[null,null]") && strstr(timing_json,"\"status\":null"));
    assert(ftu_set_data(g_ref,2,0,&a,0,NULL,sizeof frames,&frames)==kAudioHardwareUnsupportedOperationError);
    struct ftu_startup_snapshot start={0};start.header[0]=start.header[1]=1;
    start.header[2]=17;start.header[3]=5;start.header[4]=2;start.header[8]=1;
    start.header[10]=start.header[11]=2;
    start.writes[0][0]=start.writes[0][1]=1;start.writes[0][3]=3;
    start.writes[1][0]=start.writes[1][1]=1;start.writes[1][2]=(uint64_t)(int64_t)-536870168;start.writes[1][3]=UINT32_MAX;
    status=startup_snapshot_text(&start);assert(status);
    assert(CFStringGetCString(status,timing_json,sizeof timing_json,kCFStringEncodingUTF8));CFRelease(status);
    assert(strstr(timing_json,"\"status\":-536870168") && strstr(timing_json,"\"actual\":4294967295"));
    if(getenv("FTU_TEST_STARTUP_JSON"))puts(timing_json);
    struct ftu_shutdown_snapshot shut={0};shut.words[0]=shut.words[1]=1;
    char shutdown_json[4096];property_json(FTU_SHUTDOWN_PROPERTY,shutdown_json,sizeof shutdown_json);
    assert(strstr(shutdown_json,"\"present\":0") && strstr(shutdown_json,"\"native_pending\":null"));
    shut.words[2]=1;shut.words[3]=14;shut.words[12]=1;shut.words[26]=(uint64_t)(int64_t)kAudioHardwareUnspecifiedError;
    shut.words[10]=1;shut.words[28]=1;shut.words[29]=1;shut.words[30]=3;shut.words[34]=(uint64_t)(int64_t)-99;
    text_of(shutdown_snapshot_text(&shut),shutdown_json,sizeof shutdown_json);
    assert(strstr(shutdown_json,"\"epoch\":14") && strstr(shutdown_json,"\"native_pending\":0") && strstr(shutdown_json,"\"removed_mask\":3"));
    assert(strstr(shutdown_json,"\"restore_in\":-99") && strstr(shutdown_json,"\"read_alt_in\":null"));
    hotplug_regressions();
    runtime_recovery_properties();
    monitor_recovery_gating();
    monitor_recovers_before_announcing_a_transient_stop();
    set_unit(FTU_USB_NONE,0,0,0);poll_presence(false);assert(get32(2,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal)==0);
    quarantine_regression();puts("HAL boundary tests passed");return 0;
}
