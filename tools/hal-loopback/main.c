/* Bounded external CoreAudio instrument; no USB access or driver changes.
 * Precomputed mono stimulus goes only to output 3. Only selected input is retained.
 * No allocation, file IO, locks or property queries in the audio callback.
 * All other outputs are zero; input is never copied to output. */
#define main ftu_inspection_main
#include "../hal-check/main.c"
#undef main
#include <signal.h>

enum { CHANNEL = 2, RATE = 48000, MAX_SECONDS = 30, MAX_CALLBACKS = 30000 };
struct cycle {
    uint64_t raw, in_host, out_host, now_host;
    double in_sample, out_sample;
    UInt32 frames, in_flags, out_flags;
};
struct loop {
    float *plan, *capture;
    uint64_t capacity, position;
    struct cycle *cycles;
    unsigned cycle_count;
    double next_in, next_out;
    float peak;
    float rejected_sample;
    uint64_t rejected_frame;
    bool silence_only;
    unsigned input_channel;
    atomic_bool done;
    atomic_uint error;
};
static volatile sig_atomic_t interrupted;
static void interrupt_handler(int sig) { (void)sig; interrupted = 1; }
static OSStatus loop_io(AudioDeviceID device, const AudioTimeStamp *now,
    const AudioBufferList *input, const AudioTimeStamp *it, AudioBufferList *output,
    const AudioTimeStamp *ot, void *context)
{
    (void)device;
    struct loop *s = context;
    if (output) for (UInt32 b=0; b<output->mNumberBuffers; b++)
        if (output->mBuffers[b].mData)
            memset(output->mBuffers[b].mData, 0, output->mBuffers[b].mDataByteSize);
    if (atomic_load(&s->done) || atomic_load(&s->error)) return noErr;
    if (!input || !output || input->mNumberBuffers!=1 || output->mNumberBuffers!=1 ||
        !input->mBuffers[0].mData || !output->mBuffers[0].mData ||
        input->mBuffers[0].mNumberChannels!=8 || output->mBuffers[0].mNumberChannels!=8 ||
        input->mBuffers[0].mDataByteSize!=output->mBuffers[0].mDataByteSize ||
        input->mBuffers[0].mDataByteSize%32) { atomic_store(&s->error,1); return noErr; }
    UInt32 n=output->mBuffers[0].mDataByteSize/32;
    if (!n || n>4096 || s->cycle_count>=MAX_CALLBACKS) { atomic_store(&s->error,2); return noErr; }
    if (!it || !ot || !(it->mFlags&kAudioTimeStampSampleTimeValid) ||
        !(ot->mFlags&kAudioTimeStampSampleTimeValid) || !isfinite(it->mSampleTime) ||
        !isfinite(ot->mSampleTime)) { atomic_store(&s->error,3); return noErr; }
    if (s->cycle_count && (it->mSampleTime!=s->next_in || ot->mSampleTime!=s->next_out)) {
        atomic_store(&s->error,4); return noErr;
    }
    s->next_in=it->mSampleTime+n; s->next_out=ot->mSampleTime+n;
    s->cycles[s->cycle_count++]=(struct cycle){s->position,it->mHostTime,ot->mHostTime,
        now?now->mHostTime:0,it->mSampleTime,ot->mSampleTime,n,it->mFlags,ot->mFlags};
    const float *in=input->mBuffers[0].mData; float *out=output->mBuffers[0].mData;
    uint64_t count=s->capacity-s->position; if (count>n) count=n;
    for (uint64_t f=0; f<count; f++) {
        float v=in[f*8+s->input_channel];
        if (!isfinite(v) || (!s->silence_only && fabsf(v)>0.25f)) {
            s->rejected_sample=v; s->rejected_frame=s->position+f;
            atomic_store(&s->error,5); break;
        }
        if (fabsf(v)>s->peak) s->peak=fabsf(v);
        s->capture[s->position+f]=v;
    }
    if (!atomic_load(&s->error)) for (uint64_t f=0; f<count; f++)
        out[f*8+CHANNEL]=s->plan[s->position+f];
    s->position+=count;
    if (s->position>=s->capacity) atomic_store(&s->done,true);
    return noErr;
}
static AudioDeviceID find_device(void)
{
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,0};
    UInt32 size=0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&size)) return 0;
    AudioDeviceID *ids=calloc(1,size?size:1),result=0; unsigned matches=0;
    if (!ids) return 0;
    if (!AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&size,ids))
        for (unsigned i=0; i<size/sizeof(*ids); i++) {
            CFStringRef uid=string(ids[i],kAudioDevicePropertyDeviceUID);
            if (uid && CFStringHasPrefix(uid,CFSTR("net.batarelo.FastTrackUltra:"))) { result=ids[i]; matches++; }
            if (uid) CFRelease(uid);
        }
    free(ids); return matches==1?result:0;
}
static void report_buffer(AudioDeviceID dev,const char *when) {
    UInt32 n=0; OSStatus st=get(dev,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,sizeof n,&n);
    printf("buffer %s: status=%d frames=%u\n",when,st,n); fflush(stdout);
}
static OSStatus request_buffer(AudioDeviceID dev, UInt32 n, const char *when) {
    AudioObjectPropertyAddress a={kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,0};
    OSStatus st=AudioObjectSetPropertyData(dev,&a,0,NULL,sizeof n,&n);
    printf("request %s: frames=%u status=%d\n",when,n,st); report_buffer(dev,"immediate");
    for (unsigned i=0;i<50;i++) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,false);
        struct timespec pause={0,10000000}; nanosleep(&pause,NULL);
    }
    report_buffer(dev,"delayed"); return st;
}
static void declarations(AudioDeviceID dev) {
    for(unsigned d=0;d<2;d++) {
        UInt32 scope=d?kAudioObjectPropertyScopeOutput:kAudioObjectPropertyScopeInput;
        AudioObjectID stream=0; OSStatus ss=get(dev,kAudioDevicePropertyStreams,scope,sizeof stream,&stream);
        UInt32 selectors[]={kAudioDevicePropertyLatency,kAudioDevicePropertySafetyOffset,kAudioStreamPropertyLatency};
        for (unsigned p=0;p<3;p++) {
            UInt32 value=0; OSStatus st=p==2?ss:0;
            if(!st) st=get(p==2?stream:dev,selectors[p],p==2?kAudioObjectPropertyScopeGlobal:scope,sizeof value,&value);
            printf("declaration %s %s: status=%d value=%u\n",d?"output":"input",
                p==0?"device_latency":p==1?"safety":"stream_latency",st,value);
        }
    }
}
#ifndef FTU_LOOPBACK_TEST
extern int ftu_audio_authorization(void);
int main(int argc,char **argv) {
    if (argc<2 || argc>5) { fprintf(stderr,"Usage: %s RUN_DIRECTORY [BUFFER_FRAMES] [--input 3|4]\n",argv[0]);return 2; }
    int authorization=ftu_audio_authorization();
    printf("audio authorization: %d (3=authorized)\n",authorization);
    if(authorization!=3) {
        fprintf(stderr,"Microphone access is not authorized for the launching app. No audio started.\n");
        return 78;
    }
    UInt32 requested=0; unsigned input_channel=CHANNEL; bool input_selected=false;
    for(int i=2;i<argc;i++) {
        if(!strcmp(argv[i],"--input")) {
            if(input_selected || ++i>=argc || (strcmp(argv[i],"3") && strcmp(argv[i],"4"))) return 2;
            input_channel=(unsigned)(argv[i][0]-'1'); input_selected=true;
        } else {
            char *end; unsigned long n=strtoul(argv[i],&end,10);
            if(requested || !*argv[i] || *end || n<64 || n>4096) return 2; requested=(UInt32)n;
        }
    }
    char path[4096]; if (snprintf(path,sizeof path,"%s/stimulus.f32",argv[1])>=(int)sizeof path) return 2;
    FILE *f=fopen(path,"rb"); if(!f) {perror(path);return 1;}
    if(fseek(f,0,SEEK_END)) return 1; long bytes=ftell(f); rewind(f);
    if(bytes<RATE*4 || bytes>MAX_SECONDS*RATE*4 || bytes%4) {fclose(f);return 2;}
    struct loop s={0}; s.capacity=(uint64_t)bytes/4; s.input_channel=input_channel;
    s.plan=calloc(1,(size_t)bytes); s.capture=calloc(1,(size_t)bytes); s.cycles=calloc(MAX_CALLBACKS,sizeof(*s.cycles));
    if(!s.plan || !s.capture || !s.cycles) return 1;
    if(fread(s.plan,1,(size_t)bytes,f)!=(size_t)bytes) return 1; fclose(f);
    s.silence_only=true;
    for(uint64_t i=0;i<s.capacity;i++) {
        if(!isfinite(s.plan[i]) || fabsf(s.plan[i])>0.01f) return 2;
        if(s.plan[i]!=0) s.silence_only=false;
    }
    printf("stimulus: %s, output=3 input=%u other_outputs=zero\n",s.silence_only?"digital_silence":"bounded_precomputed",input_channel+1);
    AudioDeviceID dev=find_device(); if(!dev || !describe(dev)) return 1; declarations(dev);
    AudioObjectPropertyAddress timing_property={'ftuT',kAudioObjectPropertyScopeGlobal,0};
    bool timing=AudioObjectHasProperty(dev,&timing_property);
    if(timing)print_string(dev,'ftuT',"initial timing");
    UInt32 original=0; if(get(dev,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,sizeof original,&original)) return 1;
    AudioValueRange range={0}; OSStatus range_status=get(dev,kAudioDevicePropertyBufferFrameSizeRange,kAudioObjectPropertyScopeGlobal,sizeof range,&range);
    printf("buffer range: status=%d min=%.0f max=%.0f\n",range_status,range.mMinimum,range.mMaximum);
    if(requested) request_buffer(dev,requested,"before_create");
    AudioDeviceIOProcID proc=NULL; OSStatus created=AudioDeviceCreateIOProcID(dev,loop_io,&s,&proc);
    printf("create: %d\n",created);
    if(created) {if(requested) request_buffer(dev,original,"restore");return 1;}
    if(requested) request_buffer(dev,requested,"after_create");
    signal(SIGINT,interrupt_handler); signal(SIGTERM,interrupt_handler);
    OSStatus started=AudioDeviceStart(dev,proc); printf("start: %d\n",started); report_buffer(dev,"running");
    unsigned polls=0;
    if(!started) while(!atomic_load(&s.done) && !atomic_load(&s.error) && !interrupted && polls<(MAX_SECONDS+5)*10) {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,false);
        struct timespec pause={0,100000000}; nanosleep(&pause,NULL);
        if(polls%10==0) {
            print_string(dev,'ftuS',"running status");
            if(timing)print_string(dev,'ftuT',"running timing");
        }
        if(!status_is(dev,"\"phase\":2")) {atomic_store(&s.error,6);break;}
        fflush(stdout); polls++;
    }
    OSStatus stopped=started?0:AudioDeviceStop(dev,proc),destroyed=AudioDeviceDestroyIOProcID(dev,proc);
    printf("stop: %d destroy: %d interrupted: %d done: %d error: %u\n",stopped,destroyed,interrupted,atomic_load(&s.done),atomic_load(&s.error));
    print_string(dev,'ftuS',"stopped status");
    if(timing)print_string(dev,'ftuT',"stopped timing");
    if(stopped || destroyed) return 1; /* process exit owns callback storage on failed teardown */
    if(requested) request_buffer(dev,original,"restore"); report_buffer(dev,"final");
    UInt32 restored=0; bool restore_ok=!get(dev,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,sizeof restored,&restored) && restored==original;
    printf("capture: frames=%llu callbacks=%u peak=%.9g restore_ok=%d\n",(unsigned long long)s.position,s.cycle_count,s.peak,restore_ok);
    if(atomic_load(&s.error)==5) printf("rejected input: frame=%llu value=%.9g hex=%a\n",
        (unsigned long long)s.rejected_frame,s.rejected_sample,(double)s.rejected_sample);
    snprintf(path,sizeof path,"%s/capture.f32",argv[1]); f=fopen(path,"wbx"); if(!f) {perror(path);return 1;}
    bool saved=fwrite(s.capture,sizeof(float),(size_t)s.position,f)==s.position; saved &= fclose(f)==0;
    snprintf(path,sizeof path,"%s/callbacks.csv",argv[1]); f=fopen(path,"wx"); if(!f) {perror(path);return 1;}
    fprintf(f,"raw,frames,input_sample,output_sample,input_host,output_host,now_host,input_flags,output_flags\n");
    for(unsigned i=0;i<s.cycle_count;i++) { struct cycle *c=&s.cycles[i];
        fprintf(f,"%llu,%u,%.9f,%.9f,%llu,%llu,%llu,%u,%u\n",(unsigned long long)c->raw,c->frames,c->in_sample,c->out_sample,
            (unsigned long long)c->in_host,(unsigned long long)c->out_host,(unsigned long long)c->now_host,c->in_flags,c->out_flags);
    }
    saved &= !ferror(f); saved &= fclose(f)==0;
    free(s.plan); free(s.capture); free(s.cycles);
    return !saved || !restore_ok || started || interrupted || !atomic_load(&s.done) || atomic_load(&s.error);
}
#endif
