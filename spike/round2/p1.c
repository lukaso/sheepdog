#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <signal.h>
#include <spawn.h>
#include <dlfcn.h>
#include <libproc.h>
#include <sys/wait.h>
#include <stdint.h>
extern char **environ;
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a,b; uint64_t r2,r3; };
static int (*resp)(pid_t); static int (*disc)(posix_spawnattr_t*,int);
static uint64_t U(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.uniq:0; }
static uint64_t PU(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.puniq:0; }
static void rep(const char*t,pid_t p){ printf("%-34s pid=%-6d resp=%-6d uniq=%llu puniq=%llu\n",t,p,resp(p),(unsigned long long)U(p),(unsigned long long)PU(p)); fflush(stdout);}
int main(int argc,char**argv){
 alarm(20);
 resp=dlsym(RTLD_DEFAULT,"responsibility_get_pid_responsible_for_pid");
 disc=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 if(argc>1 && !strcmp(argv[1],"leaf")){ rep("  leaf (spawn, no disclaim) self",getpid()); return 0; }
 if(argc>1 && !strcmp(argv[1],"inner")){
   rep("  inner (spawn+disclaim) self",getpid());
   int fd[2]; pipe(fd); pid_t c=fork(); if(c==0){ setsid(); pid_t g=fork(); if(g==0){ execl("/bin/sleep","sleep","6",(char*)0);} write(fd[1],&g,sizeof g); _exit(0);}
   pid_t g; read(fd[0],&g,sizeof g); waitpid(c,0,0); usleep(50000);
   rep("  inner's setsid dbl-fork gc",g);
   printf("GC %d\n",g); fflush(stdout); return 0; }
 if(argc>1 && !strcmp(argv[1],"root")){
   rep(" root self",getpid());
   pid_t f=fork(); if(f==0){ rep("  fork-no-exec child",getpid()); pid_t f2=fork(); if(f2==0){ rep("  fork-no-exec grandchild",getpid()); _exit(0);} waitpid(f2,0,0); _exit(0);} waitpid(f,0,0);
   pid_t p; char*lv[]={argv[0],"leaf",NULL}; posix_spawnattr_t b; posix_spawnattr_init(&b); posix_spawn(&p,argv[0],NULL,&b,lv,environ); waitpid(p,0,0);
   posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); char*iv[]={argv[0],"inner",NULL};
   int fd[2]; pipe(fd); posix_spawn_file_actions_t fa; posix_spawn_file_actions_init(&fa); posix_spawn_file_actions_adddup2(&fa,fd[1],1);
   posix_spawn(&p,argv[0],&fa,&a,iv,environ); close(fd[1]); pid_t inner=p;
   char buf[4096]; int n=0,r; while((r=read(fd[0],buf+n,sizeof buf-1-n))>0) n+=r; buf[n]=0; waitpid(p,0,0);
   fputs(buf,stdout); pid_t gc=atoi(strstr(buf,"GC ")+3);
   rep("  gc AFTER inner exited+reaped",gc);
   printf("  inner pid was %d; gc puniq names inner uniq? see above\n",inner);
   kill(gc,9);
   pid_t z=fork(); if(z==0) _exit(0); usleep(100000);
   printf("  zombie pid=%d resp=%d uniq=%llu\n",z,resp(z),(unsigned long long)U(z)); waitpid(z,0,0);
   return 0; }
 /* supervisor */
 printf("S pid=%d resp(S)=%d\n",getpid(),resp(getpid()));
 posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); pid_t p; char*rv[]={argv[0],"root",NULL};
 posix_spawn(&p,argv[0],NULL,&a,rv,environ); waitpid(p,0,0);
 printf("other-user checks: resp(1)=%d ",resp(1));
 FILE*f=popen("ps -axo pid=,user= | awk '$2==\"root\"{print $1}' | sed -n 5p","r"); int rp=0; fscanf(f,"%d",&rp); pclose(f);
 printf("root pid %d resp=%d uniq=%llu | nonexistent pid 99998 resp=%d\n",rp,resp(rp),(unsigned long long)U(rp),resp(99998));
 const char*cands[]={"responsibility_get_uniqueid_responsible_for_pid","responsibility_get_responsible_for_pid","responsibility_get_pid_responsible_for_pid","responsibility_get_responsible_audit_token_for_pid","responsibility_spawnattrs_setdisclaim","responsibility_get_uniqueid_for_pid","responsibility_identity_for_pid",0};
 for(int i=0;cands[i];i++) printf("dlsym %-52s %s\n",cands[i],dlsym(RTLD_DEFAULT,cands[i])?"FOUND":"-");
 return 0;}
