/* External CoreAudio client. Never changes the default audio device.
 * --silence emits zeros; --tone emits five quiet left/right pairs.
 * No microphone samples are retained. The installed driver is not modified. */
#include <CoreAudio/CoreAudio.h>
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
static OSStatus get(AudioObjectID id,UInt32 sel,UInt32 scope,UInt32 size,void *out){
    AudioObjectPropertyAddress a={sel,scope,kAudioObjectPropertyElementMain};
    return AudioObjectGetPropertyData(id,&a,0,NULL,&size,out);
}
static CFStringRef string(AudioObjectID id,UInt32 selector){CFStringRef v=NULL;if(get(id,selector,kAudioObjectPropertyScopeGlobal,sizeof v,&v))return NULL;return v;}
static bool status_is(AudioObjectID id,const char *phase){
    CFStringRef s=string(id,'ftuS');char text[4096];if(!s)return false;
    bool ok=CFStringGetCString(s,text,sizeof text,kCFStringEncodingUTF8);CFRelease(s);
    return ok && strstr(text,phase) && strstr(text,"\"fault\":0") && strstr(text,"\"reason\":0");
}
static bool print_string(AudioObjectID id,UInt32 selector,const char *label){
    CFStringRef s=string(id,selector);if(!s)return false;
    CFIndex capacity=CFStringGetMaximumSizeForEncoding(CFStringGetLength(s),kCFStringEncodingUTF8)+1;
    if(capacity<1 || capacity>131072){CFRelease(s);return false;}
    char *text=malloc((size_t)capacity);if(!text){CFRelease(s);return false;}
    bool ok=CFStringGetCString(s,text,capacity,kCFStringEncodingUTF8);CFRelease(s);
    if(ok)printf("%s: %s\n",label,text);free(text);return ok;
}
enum { TONE_PERIOD=96000, TONE_LEAD=48000, TONE_PAIRS=5 };
/* Prepared before StartIO: 440 Hz left, 660 Hz right, -40 dBFS ceiling,
 * 20 ms raised-cosine ramps. Same two-second pattern as the earlier USB test. */
static float *make_tone(void){
    float *table=calloc(TONE_PERIOD*2,sizeof(float));if(!table)return NULL;
    for(unsigned frame=0;frame<TONE_PERIOD;frame++)for(unsigned ch=0;ch<2;ch++){
        unsigned begin=ch?43200:4800,end=ch?72000:33600;
        if(frame<begin || frame>=end)continue;
        unsigned distance=frame-begin;if(end-1-frame<distance)distance=end-1-frame;if(distance>960)distance=960;
        double envelope=0.5-0.5*cos(3.141592653589793*(double)distance/960.0);
        float sample=(float)(0.01*envelope*sin(6.283185307179586*(ch?660.0:440.0)*(frame-begin)/48000.0));
        if(!isfinite(sample) || fabsf(sample)>0.01f){free(table);return NULL;}
        table[frame*2+ch]=sample;
    }
    return table;
}
struct counters {
    atomic_uint_fast64_t calls,frames,invalid,gaps,emitted[2];
    double last_output,first_output;
    const float *tone;
};
static OSStatus io(AudioDeviceID device,const AudioTimeStamp *now,const AudioBufferList *input,const AudioTimeStamp *input_time,AudioBufferList *output,const AudioTimeStamp *output_time,void *context){
    (void)device;(void)now;(void)input;(void)input_time;struct counters *c=context;
    UInt32 frames=0;
    if(output)for(UInt32 b=0;b<output->mNumberBuffers;b++){
        AudioBuffer *buffer=&output->mBuffers[b];
        if(buffer->mData && buffer->mDataByteSize)memset(buffer->mData,0,buffer->mDataByteSize);
        if(buffer->mNumberChannels)frames=buffer->mDataByteSize/(sizeof(float)*buffer->mNumberChannels);
    }
    bool valid=output_time && (output_time->mFlags&kAudioTimeStampSampleTimeValid) && isfinite(output_time->mSampleTime) && output_time->mSampleTime>=0 && frames>0;
    if(!valid)atomic_fetch_add(&c->invalid,1);
    else {
        if(atomic_load(&c->calls)==0)c->first_output=output_time->mSampleTime;
        else if(output_time->mSampleTime!=c->last_output){atomic_fetch_add(&c->gaps,1);valid=false;}
        c->last_output=output_time->mSampleTime+frames;
    }
    if(c->tone && valid){
        if(output->mNumberBuffers!=1 || output->mBuffers[0].mNumberChannels!=8 || !output->mBuffers[0].mData || output->mBuffers[0].mDataByteSize!=frames*8*sizeof(float) || frames>4096){
            atomic_fetch_add(&c->invalid,1);
        }else{
            double relative=output_time->mSampleTime-c->first_output;
            if(relative>=0 && relative<TONE_LEAD+TONE_PERIOD*TONE_PAIRS && relative==(double)(uint64_t)relative){
                uint64_t first=(uint64_t)relative,nonzero[2]={0,0};float *samples=output->mBuffers[0].mData;
                for(UInt32 f=0;f<frames;f++){
                    uint64_t t=first+f;if(t<TONE_LEAD || t>=TONE_LEAD+TONE_PERIOD*TONE_PAIRS)continue;
                    unsigned pos=(unsigned)((t-TONE_LEAD)%TONE_PERIOD);
                    for(unsigned ch=0;ch<2;ch++){float value=c->tone[pos*2+ch];samples[f*8+ch]=value;nonzero[ch]+=value!=0;}
                }
                for(unsigned ch=0;ch<2;ch++)atomic_fetch_add(&c->emitted[ch],nonzero[ch]);
            }
        }
    }
    atomic_fetch_add(&c->calls,1);atomic_fetch_add(&c->frames,frames);return 0;
}
static bool describe(AudioDeviceID device){
    bool ok=print_string(device,kAudioDevicePropertyDeviceUID,"uid") && print_string(device,'ftuB',"build") && print_string(device,'ftuS',"status");
    Float64 rate=0;UInt32 buffer=0,alive=0;
    ok &= !get(device,kAudioDevicePropertyNominalSampleRate,kAudioObjectPropertyScopeGlobal,sizeof rate,&rate);
    ok &= !get(device,kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal,sizeof buffer,&buffer);
    ok &= !get(device,kAudioDevicePropertyDeviceIsAlive,kAudioObjectPropertyScopeGlobal,sizeof alive,&alive);
    printf("device: %u, alive: %u, rate: %.0f, buffer: %u\n",device,alive,rate,buffer);
    ok &= alive==1 && rate==48000 && buffer>=64 && buffer<=4096;
    AudioValueRange range={0};UInt32 period=0;
    OSStatus range_status=get(device,kAudioDevicePropertyBufferFrameSizeRange,kAudioObjectPropertyScopeGlobal,sizeof range,&range);
    OSStatus period_status=get(device,kAudioDevicePropertyZeroTimeStampPeriod,kAudioObjectPropertyScopeGlobal,sizeof period,&period);
    /* Preserve read errors: a zero initialized value is not a measurement. */
    printf("geometry: buffer_range_status=%d min=%.0f max=%.0f zts_status=%d period=%u\n",range_status,range.mMinimum,range.mMaximum,period_status,period);
    for(unsigned d=0;d<2;d++){
        UInt32 scope=d?kAudioObjectPropertyScopeOutput:kAudioObjectPropertyScopeInput;
        AudioObjectID stream=0;AudioStreamBasicDescription fmt={0};UInt32 safety=0,latency=0;
        bool got=!get(device,kAudioDevicePropertyStreams,scope,sizeof stream,&stream) && !get(stream,kAudioStreamPropertyVirtualFormat,kAudioObjectPropertyScopeGlobal,sizeof fmt,&fmt) && !get(device,kAudioDevicePropertySafetyOffset,scope,sizeof safety,&safety) && !get(device,kAudioDevicePropertyLatency,scope,sizeof latency,&latency);
        printf("%s: properties=%s, channels=%u, bytes/frame=%u, safety=%u, latency=%u\n",d?"output":"input",got?"ok":"unavailable",fmt.mChannelsPerFrame,fmt.mBytesPerFrame,safety,latency);
        ok &= got && fmt.mSampleRate==48000 && fmt.mChannelsPerFrame==8 && fmt.mBytesPerFrame==32;
    }
    return ok;
}
int main(int argc,char **argv){
    unsigned seconds=0;bool tone=false,timing=false,worker=false;
    if(argc==2 && strcmp(argv[1],"--worker-timing")==0)worker=true;
    else if(argc==2 && strcmp(argv[1],"--timing")==0)timing=true;
    else if(argc==2 && strcmp(argv[1],"--tone")==0){tone=true;seconds=12;}
    else if(argc==3 && strcmp(argv[1],"--silence")==0){char *end=NULL;unsigned long n=strtoul(argv[2],&end,10);if(!*argv[2] || *end || n<1 || n>120)return 2;seconds=(unsigned)n;}else if(argc!=1){fprintf(stderr,"Usage: %s [--silence 1..120 | --tone | --timing | --worker-timing]\n",argv[0]);return 2;}
    AudioObjectPropertyAddress a={kAudioHardwarePropertyDevices,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};UInt32 bytes=0;
    if(AudioObjectGetPropertyDataSize(kAudioObjectSystemObject,&a,0,NULL,&bytes))return 1;
    AudioDeviceID *devices=calloc(1,bytes?bytes:1);if(!devices)return 1;
    if(AudioObjectGetPropertyData(kAudioObjectSystemObject,&a,0,NULL,&bytes,devices)){free(devices);return 1;}
    AudioDeviceID selected=0;unsigned matches=0;
    for(UInt32 i=0;i<bytes/sizeof(*devices);i++){CFStringRef uid=string(devices[i],kAudioDevicePropertyDeviceUID);if(uid){if(CFStringHasPrefix(uid,CFSTR("net.batarelo.FastTrackUltra:"))){selected=devices[i];matches++;}CFRelease(uid);}}
    free(devices);if(matches!=1){fprintf(stderr,"Expected one published FastTrackUltra device; found %u.\n",matches);return 1;}
    if(!describe(selected))return 1;if(worker)return print_string(selected,'ftuW',"worker timing")?0:1;if(timing)return print_string(selected,'ftuT',"timing")?0:1;if(!seconds)return 0;
    struct counters c={0};float *table=tone?make_tone():NULL;if(tone && !table)return 1;c.tone=table;AudioDeviceIOProcID proc=NULL;
    OSStatus created=AudioDeviceCreateIOProcID(selected,io,&c,&proc);if(created){fprintf(stderr,"CreateIOProc failed: %d\n",created);free(table);return 1;}
    OSStatus started=AudioDeviceStart(selected,proc);if(started){fprintf(stderr,"Start failed: %d\n",started);print_string(selected,'ftuS',"start failure status");AudioDeviceDestroyIOProcID(selected,proc);free(table);return 1;}
    bool healthy=true;
    for(unsigned s=0;s<seconds;s++){struct timespec pause={1,0};nanosleep(&pause,NULL);if(!print_string(selected,'ftuS',"running status") || !status_is(selected,"\"phase\":2") || atomic_load(&c.invalid) || atomic_load(&c.gaps)){healthy=false;break;}fflush(stdout);}
    OSStatus stopped=AudioDeviceStop(selected,proc),destroyed=AudioDeviceDestroyIOProcID(selected,proc);
    print_string(selected,'ftuS',"stopped status");
    uint64_t calls=atomic_load(&c.calls),frames=atomic_load(&c.frames),invalid=atomic_load(&c.invalid),gaps=atomic_load(&c.gaps);
    printf("IO: calls=%llu, frames=%llu, invalid=%llu, gaps=%llu, stop=%d, destroy=%d\n",(unsigned long long)calls,(unsigned long long)frames,(unsigned long long)invalid,(unsigned long long)gaps,stopped,destroyed);
    uint64_t left=atomic_load(&c.emitted[0]),right=atomic_load(&c.emitted[1]);
    if(tone)printf("Tone: pairs=%u, peak_limit=0.01, nonzero_left=%llu, nonzero_right=%llu, other_channels=zero\n",TONE_PAIRS,(unsigned long long)left,(unsigned long long)right);
    /* Only release callback storage after both stop and IOProc destruction. */
    if(!stopped && !destroyed)free(table);
    return !healthy || !status_is(selected,"\"phase\":0") || stopped || destroyed || calls<seconds*10 || invalid || gaps || (tone && (left<140000 || right<140000)) ? 1:0;
}
