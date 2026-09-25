#include <stdio.h>
#include <unistd.h>
#include <libproc.h>
int main(int c,char**v){ char p[4096]; proc_pidpath(getpid(),p,sizeof p); printf("argv0=%s pidpath=%s\n",v[0],p); return 0;}
