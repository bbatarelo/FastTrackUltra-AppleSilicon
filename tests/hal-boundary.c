/* Actual vtable/property code, real Rust API, fake native discovery. No USB. */
#include <assert.h>
#include <math.h>
#include "../driver/plugin.c"
static bool found;
static uint32_t fake_product=0x2080;
int32_t ftu_usb_find(uint32_t *p,uint32_t *l){*p=fake_product;*l=123;return found?0:-1;}
static void *unavailable(uint32_t p,uint32_t l){(void)p;(void)l;return NULL;}
static const struct ftu_pool_ops *no_ops(void *p){(void)p;return NULL;}
static int32_t no_close(void *p){(void)p;return -1;}
static int32_t no_schedule(void *p,const struct ftu_thread_schedule *s){(void)p;(void)s;return -1;}
const struct ftu_live_factory ftu_usb_factory={unavailable,no_ops,no_close,no_schedule};
static UInt64 requested;
static OSStatus changed(AudioServerPlugInHostRef h,AudioObjectID o,UInt32 n,const AudioObjectPropertyAddress *p){(void)h;(void)o;(void)n;(void)p;return 0;}
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
    poll_presence(false);assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==0);
    found=true;poll_presence(false);assert(ftu_get_size(g_ref,1,0,&a,0,NULL,&size)==0 && size==4);
    owned_objects_qualifiers();
    assert(get32(2,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal)==1);
    assert(get32(2,kAudioDevicePropertyZeroTimeStampPeriod,kAudioObjectPropertyScopeGlobal)>=10923);
    assert(get32(2,kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeInput)==1024);
    assert(get32(2,kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeOutput)==3072);
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
    AudioServerPlugInCustomPropertyInfo props[3];
    assert(ftu_get_data(g_ref,2,0,&a,0,NULL,sizeof props,&size,props)==0 && size==sizeof props);
    assert(props[2].mSelector==FTU_TIMING_PROPERTY);
    found=false;poll_presence(false);assert(get32(2,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal)==0);
    assert(ftu_live_destroy(live_handle())==0);puts("HAL boundary tests passed");return 0;
}
