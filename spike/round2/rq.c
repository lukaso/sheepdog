#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <dlfcn.h>
int main(int c,char**v){ int (*r)(pid_t)=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 const char*out=c>1?v[1]:"/private/tmp/claude-501/dk-review2/rq.out"; FILE*f=fopen(out,"a"); fprintf(f,"pid=%d ppid=%d resp=%d\n",getpid(),getppid(),r(getpid())); fclose(f); sleep(2); return 0;}
