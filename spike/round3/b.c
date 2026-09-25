#include "common.h"
#include <time.h>
static FILE*L; static const char*D="/private/tmp/claude-501/dk-review3/b";
static void lg(const char*f,...){}
static volatile sig_atomic_t gi=0; static volatile pid_t gpid; static volatile int gcode;
static void hi(int s,siginfo_t*si,void*u){ gi++; gpid=si->si_pid; gcode=si->si_code; }
static void wpid(const char*n){ char p[256]; snprintf(p,256,"%s.%s.pid",D,n); FILE*f=fopen(p,"w"); fprintf(f,"%d\n",getpid()); fclose(f);}
int main(int argc,char**argv){ init();
 char lp[256]; snprintf(lp,256,"%s.log",D);
 if(argc>1&&!strcmp(argv[1],"esc")){ alarm(20); setsid(); wpid("esc"); for(int i=0;;i++){ FILE*f=fopen(lp,"a"); fprintf(f,"esc tick %d t=%ld\n",i,time(0)); fclose(f); usleep(500000);} }
 if(argc>1&&!strcmp(argv[1],"root")){ alarm(20); wpid("root");
   struct sigaction sa={0}; sa.sa_sigaction=hi; sa.sa_flags=SA_SIGINFO; sigaction(SIGINT,&sa,0);
   pid_t c=fork(); if(c==0){ execl(argv[0],argv[0],"esc",(char*)0); _exit(1);} 
   for(int i=0;i<40;i++){ usleep(500000); if(gi){ FILE*f=fopen(lp,"a"); fprintf(f,"root got INT x%d from pid=%d code=%d\n",gi,gpid,gcode); fclose(f); gi=0;} } return 0; }
 if(argc>1&&!strcmp(argv[1],"sup")){ alarm(25); wpid("sup");
   L=fopen(lp,"a"); setvbuf(L,0,_IONBF,0);
   fprintf(L,"sup pid=%d pgid=%d sid=%d resp=%d tcpgrp(0)=%d isatty0=%d\n",getpid(),getpgrp(),getsid(0),resp(getpid()),tcgetpgrp(0),isatty(0));
   struct sigaction sa={0}; sa.sa_sigaction=hi; sa.sa_flags=SA_SIGINFO; sigaction(SIGINT,&sa,0);
   pid_t r; char*rv[]={argv[0],"root",NULL}; posix_spawnattr_t a; posix_spawnattr_init(&a); posix_spawn(&r,argv[0],NULL,&a,rv,environ);
   for(int i=0;i<50;i++){ usleep(250000); if(gi){ fprintf(L,"sup got INT from pid=%d code=%d\n",gpid,gcode); gi=0;} if(i%4==0) fprintf(L,"sup tick %d t=%ld\n",i,time(0)); int st; if(waitpid(r,&st,WNOHANG)==r){ fprintf(L,"sup: root ended\n"); break;} }
   return 0; }
 posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); posix_spawnattr_setflags(&a,POSIX_SPAWN_SETEXEC);
 char*sv[]={argv[0],"sup",NULL}; pid_t p; posix_spawn(&p,argv[0],NULL,&a,sv,environ); return 1; }
