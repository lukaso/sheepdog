#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <sys/sysctl.h>
int main(int c,char**v){ int pid=atoi(v[1]); int mib[3]={CTL_KERN,KERN_PROCARGS2,pid}; static char b[262144]; size_t sz=sizeof b;
 int r=sysctl(mib,3,b,&sz,NULL,0); printf("r=%d sz=%zu\n",r,sz); int hit=0;
 for(size_t i=0;i+13<=sz;i++) if(!memcmp(b+i,"DEEPKILL_TAG=",13)){hit=1;break;} printf("tag=%d\n",hit); return 0;}
