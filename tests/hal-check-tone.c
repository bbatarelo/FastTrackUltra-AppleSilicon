/* Exercise the actual diagnostic IOProc without opening a device. */
#define main hal_check_cli_main
#include "../tools/hal-check/main.c"
#undef main
#include <assert.h>

int main(void){
    float *table=make_tone();assert(table);
    struct counters c={0};c.tone=table;
    float samples[192*8];
    AudioBufferList output={.mNumberBuffers=1,.mBuffers={{8,sizeof samples,samples}}};
    AudioTimeStamp timestamp={.mSampleTime=10000,.mFlags=kAudioTimeStampSampleTimeValid};
    uint64_t nonzero[2]={0,0};
    unsigned crossings[2]={0,0};
    for(unsigned frame=0;frame<48000*12;frame+=192){
        timestamp.mSampleTime=10000+frame;
        memset(samples,0x7f,sizeof samples);
        assert(io(0,NULL,NULL,NULL,&output,&timestamp,&c)==0);
        for(unsigned f=0;f<192;f++)for(unsigned ch=0;ch<8;ch++){
            float value=samples[f*8+ch];
            assert(isfinite(value) && fabsf(value)<=0.01f);
            unsigned t=frame+f;
            if(ch>=2 || t<TONE_LEAD || t>=TONE_LEAD+TONE_PERIOD*TONE_PAIRS){assert(value==0);continue;}
            unsigned pos=(t-TONE_LEAD)%TONE_PERIOD;
            unsigned begin=ch?43200:4800,end=ch?72000:33600;
            if(pos<=begin || pos>=end-1)assert(value==0);
            nonzero[ch]+=value!=0;
        }
    }
    for(unsigned ch=0;ch<2;ch++){
        unsigned begin=(ch?43200:4800)+2400;
        for(unsigned f=begin;f<begin+24000;f++)if(table[f*2+ch]<=0 && table[(f+1)*2+ch]>0)crossings[ch]++;
        assert(crossings[ch]>=(ch?329:219) && crossings[ch]<=(ch?331:221));
        assert(nonzero[ch]>140000 && nonzero[ch]<=144000);
        assert(atomic_load(&c.emitted[ch])==nonzero[ch]);
    }
    assert(atomic_load(&c.calls)==3000 && atomic_load(&c.frames)==576000);
    assert(atomic_load(&c.invalid)==0 && atomic_load(&c.gaps)==0);
    /* A discontinuity or invalid time produces silence. */
    timestamp.mSampleTime+=384;
    io(0,NULL,NULL,NULL,&output,&timestamp,&c);
    assert(atomic_load(&c.gaps)==1);
    for(unsigned i=0;i<192*8;i++)assert(samples[i]==0);
    timestamp.mSampleTime=NAN;
    io(0,NULL,NULL,NULL,&output,&timestamp,&c);
    assert(atomic_load(&c.invalid)==1);
    for(unsigned i=0;i<192*8;i++)assert(samples[i]==0);
    /* Reject the wrong interleaved channel count, with no nonzero output. */
    struct counters malformed={0};malformed.tone=table;
    output.mBuffers[0].mNumberChannels=4;
    timestamp.mSampleTime=10000;
    io(0,NULL,NULL,NULL,&output,&timestamp,&malformed);
    assert(atomic_load(&malformed.invalid)==1);
    for(unsigned i=0;i<192*8;i++)assert(samples[i]==0);
    printf("PASS: 12 s callback sequence, five pairs, 440/660 Hz, -40 dBFS ceiling, six silent channels, bounded windows, invalid/gap silence; nonzero=%llu/%llu\n",(unsigned long long)nonzero[0],(unsigned long long)nonzero[1]);
    free(table);return 0;
}
