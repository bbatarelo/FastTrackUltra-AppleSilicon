/* Hardware exerciser for the production session API. Output is always silence. */
#include "../../shared/ftu_live.h"
#include <mach/mach_time.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec*1e-9;}
int main(int argc,char **argv){
    unsigned seconds=argc>1?(unsigned)strtoul(argv[1],NULL,10):10;
    unsigned cycles=argc>2?(unsigned)strtoul(argv[2],NULL,10):1;
    if(argc>3 || !seconds || seconds>120 || !cycles || cycles>3)return 2;
    uint32_t product=0,location=0;if(ftu_usb_find(&product,&location)){fprintf(stderr,"Exactly one supported USB interface is required.\n");return 1;}
    struct ftu_live *h=ftu_live_create(&ftu_usb_factory,product,location);if(!h)return 1;
    struct ftu_live_config cfg;ftu_live_configuration(&cfg);
    printf("{\"event\":\"configuration\",\"product\":%u,\"location\":%u,\"channels\":%u,\"rate\":%u,\"period\":%u,\"ring_frames\":%u,\"output_safety\":%u,\"input_safety\":%u}\n",product,location,cfg.channels,cfg.rate,cfg.period,cfg.ring_frames,cfg.output_safety,cfg.input_safety);fflush(stdout);
    for(unsigned cycle=0;cycle<cycles;cycle++){
        int start=ftu_live_start(h,1);struct ftu_live_status s={0};ftu_live_status(h,&s);
        printf("{\"event\":\"start\",\"cycle\":%u,\"result\":%d,\"phase\":%u,\"epoch\":%llu,\"fault\":%d,\"reason\":%u}\n",cycle+1,start,s.phase,(unsigned long long)s.epoch,s.fault,s.reason);fflush(stdout);
        if(start){ftu_live_stop(h,1);return 1;}
        double began=now();uint64_t cursor=0,updates=0,writes=0,reads=0,baseline_missing=0;bool baseline=false;int failure=0;
        struct ftu_live_timestamp previous={0};float silence[1024*8]={0},input[1024*8];
        while(now()-began<seconds){
            struct ftu_live_timestamp z;int r=ftu_live_timestamp(h,&z);
            if(r){failure=10;break;}
            if(previous.epoch && (z.epoch!=previous.epoch || z.frame<previous.frame || z.ticks<previous.ticks || (z.frame==previous.frame && z.ticks!=previous.ticks))){failure=11;break;}
            if(z.frame!=previous.frame)updates++;
            previous=z;
            if(!cursor)cursor=z.frame+cfg.output_safety;
            uint64_t end=z.frame+cfg.output_safety+1024;
            while(cursor<end){uint32_t n=(uint32_t)((end-cursor)>1024?1024:end-cursor);if(ftu_live_write(h,cursor,n,silence)){failure=12;break;}cursor+=n;writes+=n;}
            if(failure)break;
            if(z.frame>cfg.input_safety){if(ftu_live_read(h,z.frame-cfg.input_safety,256,input)){failure=13;break;}for(unsigned i=0;i<256*8;i++)if(!isfinite(input[i]) || fabsf(input[i])>1){failure=14;break;}reads+=256;}
            ftu_live_status(h,&s);
            if(s.phase!=2){failure=15;break;}
            if(!baseline && now()-began>=1){baseline_missing=s.playback_missing;baseline=true;}
            struct timespec pause={0,2000000};nanosleep(&pause,NULL);
        }
        ftu_live_status(h,&s);uint64_t live_missing=s.playback_missing;
        int stop=ftu_live_stop(h,1);ftu_live_status(h,&s);
        printf("{\"event\":\"stop\",\"cycle\":%u,\"elapsed_seconds\":%.6f,\"result\":%d,\"check_error\":%d,\"phase\":%u,\"epoch\":%llu,\"clock_updates\":%llu,\"written_frames\":%llu,\"read_frames\":%llu,\"captured\":%llu,\"prepared\":%llu,\"playback_missing\":%llu,\"steady_missing\":%llu,\"capture_missing\":%llu,\"submitted\":[%llu,%llu],\"completed\":[%llu,%llu],\"pending\":[%llu,%llu],\"cleanup\":%d,\"fault\":%d,\"reason\":%u}\n",cycle+1,now()-began,stop,failure,s.phase,(unsigned long long)s.epoch,(unsigned long long)updates,(unsigned long long)writes,(unsigned long long)reads,(unsigned long long)s.captured,(unsigned long long)s.prepared,(unsigned long long)s.playback_missing,(unsigned long long)(baseline?live_missing-baseline_missing:0),(unsigned long long)s.capture_missing,(unsigned long long)s.submitted[0],(unsigned long long)s.submitted[1],(unsigned long long)s.completed[0],(unsigned long long)s.completed[1],(unsigned long long)s.pending[0],(unsigned long long)s.pending[1],s.cleanup_status,s.fault,s.reason);fflush(stdout);
        if(s.reason==20 || s.reason==26 || s.reason==29)printf("{\"event\":\"submit_failure\",\"status\":%d,\"direction\":%u,\"sequence\":%llu,\"start\":%llu,\"checked\":%llu,\"returned\":%llu}\n",s.submit_status,s.submit_direction,(unsigned long long)s.submit_sequence,(unsigned long long)s.submit_start,(unsigned long long)s.submit_checked,(unsigned long long)s.submit_returned);
        if(stop || failure || s.pending[0] || s.pending[1] || s.cleanup_status || s.fault || s.capture_missing || (baseline && live_missing!=baseline_missing))return 1;
    }
    return ftu_live_destroy(h)?1:0;
}
