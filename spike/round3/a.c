#include "common.h"
static void state(const char*t){
  struct sigaction sa; sigset_t m; sigprocmask(0,NULL,&m);
  printf("[%s] pid=%d ppid=%d pgid=%d sid=%d resp=%d uniq=%llu isatty0=%d tcpgrp=%d ",t,getpid(),getppid(),getpgrp(),getsid(0),resp(getpid()),(unsigned long long)U(getpid()),isatty(0),isatty(0)?tcgetpgrp(0):-1);
  for(int fd=0;fd<10;fd++){ struct stat st; if(fstat(fd,&st)==0) printf("fd%d:ino%llu%s ",fd,(unsigned long long)st.st_ino,(fcntl(fd,F_GETFD)&FD_CLOEXEC)?"(cx)":""); }
  sigaction(SIGQUIT,NULL,&sa); printf("QUIT=%s ",sa.sa_handler==SIG_IGN?"IGN":sa.sa_handler==SIG_DFL?"DFL":"H");
  sigaction(SIGUSR2,NULL,&sa); printf("USR2=%s ",sa.sa_handler==SIG_IGN?"IGN":sa.sa_handler==SIG_DFL?"DFL":"H");
  printf("USR1blocked=%d env=%s\n",sigismember(&m,SIGUSR1),getenv("DK_MARK")?getenv("DK_MARK"):"-"); fflush(stdout);
}
static void h(int s){}
int main(int argc,char**argv){ init(); alarm(10);
  if(argc>1&&!strcmp(argv[1],"sup")){ state("after"); return 42; }
  state("before");
  int fd7=open("/dev/null",O_RDONLY); dup2(fd7,7); close(fd7);
  int fd8=open("/dev/null",O_RDONLY|O_CLOEXEC); if(fd8!=8){dup2(fd8,8); fcntl(8,F_SETFD,FD_CLOEXEC); close(fd8);}
  signal(SIGQUIT,SIG_IGN); signal(SIGUSR2,h); sigset_t m; sigemptyset(&m); sigaddset(&m,SIGUSR1); sigprocmask(SIG_BLOCK,&m,NULL);
  state("before2");
  posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); posix_spawnattr_setflags(&a,POSIX_SPAWN_SETEXEC);
  char*sv[]={argv[0],"sup",NULL}; pid_t p; int rc=posix_spawn(&p,argv[0],NULL,&a,sv,environ);
  printf("SETEXEC failed rc=%d\n",rc); return 1; }
