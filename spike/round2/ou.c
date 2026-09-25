#include <stdio.h>
#include <stdlib.h>
#include <dlfcn.h>
#include <unistd.h>
int main(int c,char**v){ int (*r)(pid_t)=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid"); for(int i=1;i<c;i++){int p=atoi(v[i]); printf("pid %d resp=%d\n",p,r(p));} return 0;}
