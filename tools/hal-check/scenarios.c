/* Real CoreAudio two-IOProc test, with optional buffer-size read-back/restore.
 * Only emits silence. Reuses the audited external-client callback. */
#define main hal_check_original_main
#include "main.c"
#undef main

static AudioDeviceID find_device(void){
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,0};
    UInt32 bytes=0;
    if(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&bytes))return 0;
    AudioDeviceID *ids=calloc(1,bytes?bytes:1);if(!ids)return 0;
    if(AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&bytes,ids)){free(ids);return 0;}
    AudioDeviceID selected=0;unsigned count=0;
    for(UInt32 i=0;i<bytes/sizeof(*ids);i++){
        CFStringRef uid=string(ids[i],kAudioDevicePropertyDeviceUID);
        if(uid){if(CFStringHasPrefix(uid,CFSTR("net.batarelo.FastTrackUltra:"))){selected=ids[i];count++;}CFRelease(uid);}
    }
    free(ids);return count==1?selected:0;
}
static bool buffer_set(AudioDeviceID device,UInt32 requested){
    AudioObjectPropertyAddress a={kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,0};
    OSStatus rc=AudioObjectSetPropertyData(device,&a,0,NULL,sizeof requested,&requested);
    UInt32 actual=0;
    for(unsigned n=0;n<200;n++){
        /* HAL notifications use this command-line process's main run loop.
         * Service it before reading back an asynchronous property change. */
        CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,true);
        if(!get(device,a.mSelector,a.mScope,sizeof actual,&actual) && actual==requested)break;
        usleep(10000);
    }
    printf("buffer: requested=%u, actual=%u, set_status=%d\n",requested,actual,rc);
    return rc==0 && actual==requested;
}
static bool observe(AudioDeviceID device,const char *stage,unsigned seconds,struct counters *a,struct counters *b){
    for(unsigned s=0;s<seconds;s++){
        sleep(1);printf("stage: %s\n",stage);
        if(!print_string(device,'ftuS',"status") || !status_is(device,"\"phase\":2") ||
           atomic_load(&a->invalid) || atomic_load(&a->gaps) || atomic_load(&b->invalid) || atomic_load(&b->gaps))return false;
        fflush(stdout);
    }
    return true;
}
int main(int argc,char **argv){
    UInt32 requested=0;unsigned hold=10;
    if(argc!=1 && argc!=3 && argc!=5)return 2;
    if(argc>=3){char *end=NULL;unsigned long n=strtoul(argv[2],&end,10);if(strcmp(argv[1],"--buffer") || *end || n<64 || n>1024)return 2;requested=(UInt32)n;}
    if(argc==5){char *end=NULL;unsigned long n=strtoul(argv[4],&end,10);if(strcmp(argv[3],"--hold") || *end || n<10 || n>120)return 2;hold=(unsigned)n;}
    AudioDeviceID device=find_device();if(!device || !describe(device) || !status_is(device,"\"phase\":0"))return 1;
    UInt32 original=0;if(get(device,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,sizeof original,&original))return 1;
    struct counters a={0},b={0};AudioDeviceIOProcID proc[2]={NULL,NULL};bool active[2]={false,false},ok=false;
    OSStatus stopped[2]={0,0},destroyed[2]={0,0};
    if(requested && !buffer_set(device,requested))goto cleanup;
    if(AudioDeviceCreateIOProcID(device,io,&a,&proc[0]) || AudioDeviceCreateIOProcID(device,io,&b,&proc[1]))goto cleanup;
    if(AudioDeviceStart(device,proc[0]))goto cleanup;active[0]=true;
    if(!observe(device,"A only",5,&a,&b))goto cleanup;
    if(AudioDeviceStart(device,proc[1]))goto cleanup;active[1]=true;
    if(!observe(device,"A and B",5,&a,&b))goto cleanup;
    stopped[0]=AudioDeviceStop(device,proc[0]);if(stopped[0])goto cleanup;active[0]=false;
    destroyed[0]=AudioDeviceDestroyIOProcID(device,proc[0]);if(destroyed[0])goto cleanup;proc[0]=NULL;
    uint64_t before=atomic_load(&b.calls);
    if(!observe(device,"B after A stopped",hold,&a,&b) || atomic_load(&b.calls)<=before)goto cleanup;
    ok=true;
cleanup:
    for(unsigned i=0;i<2;i++){
        if(active[i])stopped[i]=AudioDeviceStop(device,proc[i]);
        if(proc[i])destroyed[i]=AudioDeviceDestroyIOProcID(device,proc[i]);
        if(stopped[i] || destroyed[i])ok=false;
    }
    print_string(device,'ftuS',"stopped status");
    if(!status_is(device,"\"phase\":0"))ok=false;
    if(requested && !buffer_set(device,original))ok=false;
    printf("A: calls=%llu, frames=%llu, invalid=%llu, gaps=%llu, stop=%d, destroy=%d\n",(unsigned long long)atomic_load(&a.calls),(unsigned long long)atomic_load(&a.frames),(unsigned long long)atomic_load(&a.invalid),(unsigned long long)atomic_load(&a.gaps),stopped[0],destroyed[0]);
    printf("B: calls=%llu, frames=%llu, invalid=%llu, gaps=%llu, stop=%d, destroy=%d\n",(unsigned long long)atomic_load(&b.calls),(unsigned long long)atomic_load(&b.frames),(unsigned long long)atomic_load(&b.invalid),(unsigned long long)atomic_load(&b.gaps),stopped[1],destroyed[1]);
    puts(ok?"PASS":"FAIL");return ok?0:1;
}
