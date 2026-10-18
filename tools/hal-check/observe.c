/* Read-only property observer: never creates an IOProc or opens USB.
 * Status is sampled at 20 Hz, timing at 1 Hz. Host ticks bracket each query.
 * Cumulative per-epoch counters help bound changes to UI action intervals;
 * sampling does not reveal every HAL call or attribute a change to one client. */
#define main ftu_inspection_main
#include "main.c"
#undef main
#include <mach/mach_time.h>
#include <signal.h>
static volatile sig_atomic_t stopped;
static void stop_observer(int signal) { (void)signal; stopped=1; }
static AudioDeviceID select_device(void) {
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,0};
    UInt32 bytes=0; if(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&bytes))return 0;
    AudioDeviceID *ids=calloc(1,bytes?bytes:1),chosen=0;unsigned matches=0;
    if(!ids)return 0;
    if(!AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&bytes,ids))
        for(unsigned i=0;i<bytes/sizeof(*ids);i++) {
            CFStringRef uid=string(ids[i],kAudioDevicePropertyDeviceUID);
            if(uid && CFStringHasPrefix(uid,CFSTR("net.batarelo.FastTrackUltra:"))){chosen=ids[i];matches++;}
            if(uid)CFRelease(uid);
        }
    free(ids);return matches==1?chosen:0;
}
static bool snapshot(AudioDeviceID dev,UInt32 selector,const char *kind) {
    uint64_t before=mach_absolute_time(); CFStringRef s=string(dev,selector);
    uint64_t after=mach_absolute_time(); char text[131072];
    if(!s)return false;
    bool ok=CFStringGetCString(s,text,sizeof text,kCFStringEncodingUTF8);CFRelease(s);
    if(ok)printf("{\"kind\":\"%s\",\"before_ticks\":%llu,\"after_ticks\":%llu,\"data\":%s}\n",kind,(unsigned long long)before,(unsigned long long)after,text);
    return ok;
}
int main(int argc,char **argv) {
    if(argc!=2){fprintf(stderr,"Usage: %s SECONDS (1..1800)\n",argv[0]);return 2;}
    char *end=NULL;unsigned long seconds=strtoul(argv[1],&end,10);
    if(!*argv[1] || *end || seconds<1 || seconds>1800)return 2;
    AudioDeviceID dev=select_device();if(!dev)return 1;
    CFStringRef build=string(dev,'ftuB');char id[256];
    if(!build)return 1;
    bool ok=CFStringGetCString(build,id,sizeof id,kCFStringEncodingUTF8);CFRelease(build);if(!ok)return 1;
    mach_timebase_info_data_t tb;mach_timebase_info(&tb);
    printf("{\"kind\":\"identity\",\"build\":\"%s\",\"timebase_numer\":%u,\"timebase_denom\":%u,\"seconds\":%lu}\n",id,tb.numer,tb.denom,seconds);
    signal(SIGTERM,stop_observer);signal(SIGINT,stop_observer);
    uint64_t start=mach_absolute_time();unsigned polls=0;
    while(!stopped && (double)(mach_absolute_time()-start)*tb.numer/tb.denom/1e9<seconds) {
        if(!snapshot(dev,'ftuS',"status"))return 1;
        if(polls%20==0 && !snapshot(dev,'ftuT',"timing"))return 1;
        fflush(stdout);polls++;
        struct timespec pause={0,50000000};nanosleep(&pause,NULL);
    }
    if(!snapshot(dev,'ftuS',"final_status") || !snapshot(dev,'ftuT',"final_timing") || !snapshot(dev,'ftuW',"final_worker"))return 1;
    printf("{\"kind\":\"end\",\"polls\":%u,\"interrupted\":%s}\n",polls,stopped?"true":"false");
    return 0;
}
