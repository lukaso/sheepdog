// B: orphan test. S=setsid leader; J=new group G under S (G not orphaned); K=child of J in G (parent SAME group).
// K's plan-rule check (parent same session, different group) says "maybe orphaned" -> would not stop.
// Kernel verdict: K raises TSTP with default disposition; a SIGCONT handler tells stop vs discard.
#define _XOPEN_SOURCE 700
#define _DARWIN_C_SOURCE
#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <sys/wait.h>
static volatile sig_atomic_t gotcont=0, gothup=0; static void hc(int s){ if(s==SIGCONT) gotcont=1; else gothup=1; }
static void probe(const char*tag){ pid_t pp=getppid();
 int rule = (getsid(pp)==getsid(0) && getpgid(pp)!=getpgrp());
 signal(SIGCONT,hc); signal(SIGTSTP,SIG_DFL); gotcont=0; raise(SIGTSTP);
 printf("%s: pid=%d pgid=%d ppid=%d ppgid=%d rule_says_not_orphaned=%d kernel_stopped=%d\n",tag,getpid(),getpgrp(),pp,getpgid(pp),rule,(int)gotcont); fflush(stdout);}
int main(){ setvbuf(stdout,0,_IONBF,0);
 pid_t S=fork(); if(!S){ setsid();
   pid_t J=fork(); if(!J){ setpgid(0,0);
     pid_t K=fork(); if(!K){ probe("K(parent in same group)"); _exit(0);} 
     int st; for(;;){ pid_t r=waitpid(K,&st,WUNTRACED); if(r==K&&WIFSTOPPED(st)){ printf("  J saw K stopped -> CONT\n"); usleep(100000); kill(K,SIGCONT); continue;} break;}
     probe("J(parent in other group, same session)");
     _exit(0);} 
   int st; for(;;){ pid_t r=waitpid(J,&st,WUNTRACED); if(r==J&&WIFSTOPPED(st)){ printf("  S saw J stopped -> CONT\n"); usleep(100000); kill(J,SIGCONT); continue;} break;}
   probe("S(session leader, orphaned group)"); _exit(0);} 
 int st; waitpid(S,&st,0);
 // C: group becomes orphaned while stopped
 pid_t S2=fork(); if(!S2){ setsid(); pid_t J2=fork(); if(!J2){ setpgid(0,0); signal(SIGHUP,hc); signal(SIGCONT,hc);
      kill(getpid(),SIGSTOP); printf("C: J2 resumed: gothup=%d gotcont=%d ppid=%d\n",(int)gothup,(int)gotcont,getppid()); _exit(0);} 
   usleep(300000); _exit(0);} 
 waitpid(S2,&st,0); usleep(700000); return 0; }
