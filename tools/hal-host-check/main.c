/* Load the real bundle in a standalone host and exercise its vtable with silence.
 * This is not evidence that Apple's sandboxed audio host loaded the driver. */
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <dlfcn.h>
#include <math.h>
#include <mach/mach_time.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
static AudioServerPlugInDriverRef driver;
static OSStatus changed(AudioServerPlugInHostRef h,AudioObjectID id,UInt32 n,const AudioObjectPropertyAddress *p){(void)h;(void)id;(void)n;(void)p;return 0;}
static OSStatus request(AudioServerPlugInHostRef h,AudioObjectID id,UInt64 action,void *info){(void)h;return (*driver)->PerformDeviceConfigurationChange(driver,id,action,info);}
static OSStatus get(AudioObjectID obj,UInt32 selector,UInt32 scope,UInt32 bytes,void *out){AudioObjectPropertyAddress a={selector,scope,0};UInt32 actual=0;return (*driver)->GetPropertyData(driver,obj,getpid(),&a,0,NULL,bytes,&actual,out);}
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
static void diagnostic(const char *event){
    CFStringRef value=NULL;if(get(2,'ftuS',kAudioObjectPropertyScopeGlobal,sizeof value,&value)){puts("{\"event\":\"diagnostic_unavailable\"}");return;}
    char text[2048];if(CFStringGetCString(value,text,sizeof text,kCFStringEncodingUTF8))printf("{\"event\":\"%s\",\"status\":%s}\n",event,text);CFRelease(value);fflush(stdout);
}
int main(int argc,char **argv){
    const char *path=argc>1?argv[1]:"build/FastTrackUltra.driver/Contents/MacOS/FastTrackUltra";
    unsigned seconds=argc>2?(unsigned)strtoul(argv[2],NULL,10):10;if(!seconds || seconds>120)return 2;
    void *bundle=dlopen(path,RTLD_NOW|RTLD_LOCAL);if(!bundle){fprintf(stderr,"%s\n",dlerror());return 1;}
    void *(*factory)(CFAllocatorRef,CFUUIDRef)=dlsym(bundle,"FTUFactory");if(!factory)return 1;
    driver=factory(NULL,kAudioServerPlugInTypeUUID);if(!driver)return 1;
    AudioServerPlugInHostInterface host={.PropertiesChanged=changed,.RequestDeviceConfigurationChange=request};
    if((*driver)->Initialize(driver,&host))return 1;
    AudioObjectID devices[1]={0};if(get(1,kAudioPlugInPropertyDeviceList,kAudioObjectPropertyScopeGlobal,sizeof devices,devices) || devices[0]!=2)return 1;
    CFStringRef build=NULL;if(get(2,'ftuB',kAudioObjectPropertyScopeGlobal,sizeof build,&build))return 1;
    char identity[128];if(!CFStringGetCString(build,identity,sizeof identity,kCFStringEncodingUTF8))return 1;CFRelease(build);
    AudioStreamBasicDescription format;if(get(4,kAudioStreamPropertyVirtualFormat,kAudioObjectPropertyScopeGlobal,sizeof format,&format))return 1;
    if(format.mSampleRate!=48000 || format.mChannelsPerFrame!=8 || format.mBytesPerFrame!=32)return 1;
    UInt32 period=0;
    if(get(2,kAudioDevicePropertyZeroTimeStampPeriod,kAudioObjectPropertyScopeGlobal,sizeof period,&period) || !period)return 1;
    mach_timebase_info_data_t timebase;if(mach_timebase_info(&timebase)!=KERN_SUCCESS)return 1;
    UInt32 safety=0;if(get(2,kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeOutput,4,&safety))return 1;
    printf("{\"event\":\"loaded\",\"build\":\"%s\",\"rate\":%.0f,\"channels\":%u,\"safety\":%u}\n",identity,format.mSampleRate,format.mChannelsPerFrame,safety);fflush(stdout);
    AudioServerPlugInClientInfo client={.mClientID=77,.mProcessID=getpid()};if((*driver)->AddDeviceClient(driver,2,&client))return 1;
    if((*driver)->StartIO(driver,2,77)){diagnostic("start_failed");return 1;}diagnostic("started");
    double began=now();Float64 previous_frame=-1;UInt64 previous_ticks=0,previous_seed=0;uint64_t cursor=0,updates=0;int error=0;float zero[1024*8]={0},input[1024*8];
    while(now()-began<seconds){
        Float64 frame=0;UInt64 ticks=0,seed=0;if((*driver)->GetZeroTimeStamp(driver,2,77,&frame,&ticks,&seed)){error=1;break;}
        if(previous_frame>=0 && (frame<previous_frame || ticks<previous_ticks || seed!=previous_seed || (frame==previous_frame && ticks!=previous_ticks))){error=2;break;}
        if(frame!=previous_frame)updates++;previous_frame=frame;previous_ticks=ticks;previous_seed=seed;
        /* Emulate a host progressing between held timestamp pairs. Nominal
         * rate here drives diagnostic silence only; it is not a clock fit. */
        uint64_t host=mach_absolute_time();if(host<ticks){error=5;break;}
        double elapsed=(double)(host-ticks)*timebase.numer/timebase.denom*1e-9;
        uint64_t current=(uint64_t)(frame+floor(elapsed*format.mSampleRate));
        if(!cursor)cursor=current+safety;
        uint64_t end=current+safety+1024;
        while(cursor<end){UInt32 n=(UInt32)((end-cursor)>1024?1024:end-cursor);AudioServerPlugInIOCycleInfo cycle={0};cycle.mOutputTime.mSampleTime=(Float64)cursor;cycle.mOutputTime.mFlags=kAudioTimeStampSampleTimeValid;
            if((*driver)->DoIOOperation(driver,2,4,77,kAudioServerPlugInIOOperationWriteMix,n,&cycle,zero,NULL)){error=3;break;}cursor+=n;}
        if(error)break;
        if(current>1024){AudioServerPlugInIOCycleInfo cycle={0};cycle.mInputTime.mSampleTime=(Float64)(current-1024);cycle.mInputTime.mFlags=kAudioTimeStampSampleTimeValid;if((*driver)->DoIOOperation(driver,2,3,77,kAudioServerPlugInIOOperationReadInput,256,&cycle,input,NULL)){error=4;break;}}
        struct timespec pause={0,2000000};nanosleep(&pause,NULL);
    }
    diagnostic("before_stop");int stop=(*driver)->StopIO(driver,2,77);diagnostic("stopped");
    printf("{\"event\":\"result\",\"check_error\":%d,\"stop\":%d,\"clock_updates\":%llu,\"elapsed\":%.6f}\n",error,stop,(unsigned long long)updates,now()-began);fflush(stdout);
    /* Do not dlclose: the plug-in's process-lifetime discovery monitor is live. */
    return error || stop || updates+1<(uint64_t)(seconds*format.mSampleRate/period)?1:0;
}
