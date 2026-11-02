/* Verify the actually linked Rust profile; does not construct/open audio. */
#include "../shared/ftu_native.h"
#include <stdio.h>
#include <stdlib.h>
int main(int argc,char **argv)
{
    _Static_assert(sizeof(struct ftu_transport_geometry)==24,"transport geometry ABI");
    struct ftu_transport_geometry g={0};
    if((argc!=3 && argc!=4 && argc!=5) || ftu_live_transport_geometry(NULL)!=-1 || ftu_live_transport_geometry(&g)!=0)return 1;
    if(g.version!=1 || g.entries!=(unsigned)atoi(argv[1])*8 || g.slots!=(unsigned)atoi(argv[2]) || g.replay_ms!=64 || g.pcm_offset!=3072)return 2;
    unsigned policy=ftu_live_output_policy();
    if(policy!=(argc>=4?(unsigned)atoi(argv[3]):0))return 4;
    unsigned order=ftu_live_submission_order();
    if(order!=(argc==5?(unsigned)atoi(argv[4]):0))return 5;
    unsigned history=1,needed=64/(g.entries/8)+2*g.slots+1;
    while(history<needed)history*=2;if(g.history!=history)return 3;
    printf("{\"version\":%u,\"entries\":%u,\"slots\":%u,\"history\":%u,\"replay_ms\":%u,\"pcm_offset\":%u,\"output_wait_ms\":%u,\"submission_order\":%u}\n",g.version,g.entries,g.slots,g.history,g.replay_ms,g.pcm_offset,policy,order);
    return 0;
}
