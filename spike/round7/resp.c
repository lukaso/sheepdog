#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>
#include <libproc.h>
int main(int c,char**v){ int (*r)(pid_t)=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid"); for(int i=1;i<c;i++){int p=atoi(v[i]); int q=r(p); char path[4096]="?"; proc_pidpath(q,path,sizeof path); printf("pid %d resp=%d %s\n",p,q,path);} return 0;}
