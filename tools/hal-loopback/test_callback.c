#define FTU_LOOPBACK_TEST
#include "main.c"
#include <assert.h>

int main(void) {
    float plan[4]={0.001f,-0.002f,0.003f,0},capture[4]={0};
    float in[16]={0},out[16]; struct cycle cycles[4]={0};
    struct loop s={.plan=plan,.capture=capture,.capacity=4,.cycles=cycles,.input_channel=2};
    AudioBufferList ib={.mNumberBuffers=1,.mBuffers={{8,sizeof in,in}}};
    AudioBufferList ob={.mNumberBuffers=1,.mBuffers={{8,sizeof out,out}}};
    AudioTimeStamp it={.mSampleTime=-1024,.mFlags=kAudioTimeStampSampleTimeValid};
    AudioTimeStamp ot={.mSampleTime=4480,.mFlags=kAudioTimeStampSampleTimeValid};
    in[2]=0.01f;in[10]=-0.02f;
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    assert(!atomic_load(&s.error) && s.position==2 && s.cycle_count==1);
    assert(capture[0]==in[2] && capture[1]==in[10]);
    for(unsigned i=0;i<16;i++) assert(out[i]==(i==2?plan[0]:i==10?plan[1]:0));
    /* A gap cannot emit stale or newly scheduled audio. */
    it.mSampleTime+=3;ot.mSampleTime+=2;
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    assert(atomic_load(&s.error)==4);
    for(unsigned i=0;i<16;i++) assert(out[i]==0);
    /* High return level aborts this whole output callback. */
    s=(struct loop){.plan=plan,.capture=capture,.capacity=4,.cycles=cycles,.input_channel=2};
    in[10]=0.26f;
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    assert(atomic_load(&s.error)==5);
    for(unsigned i=0;i<16;i++) assert(out[i]==0);
    /* A complete finite plan cannot play again on another callback. */
    s=(struct loop){.plan=plan,.capture=capture,.capacity=2,.cycles=cycles,.input_channel=2};in[10]=0;
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    assert(atomic_load(&s.done));
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    for(unsigned i=0;i<16;i++) assert(out[i]==0);
    /* Cross-channel capture is independent of the fixed output channel. */
    s=(struct loop){.plan=plan,.capture=capture,.capacity=2,.cycles=cycles,.input_channel=3};
    in[3]=0.03f;in[11]=-0.04f;
    loop_io(0,NULL,&ib,&it,&ob,&ot,&s);
    assert(capture[0]==in[3] && capture[1]==in[11]);
    assert(out[2]==plan[0] && out[3]==0 && out[10]==plan[1] && out[11]==0);
    puts("callback channel routing, gap silence, level cutoff and finite stop: PASS");
    return 0;
}
