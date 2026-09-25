#include <stdio.h>
#include <spawn.h>
#include <dlfcn.h>
#include <sys/wait.h>
extern char **environ;
int main(int c,char**v){ int (*disc)(posix_spawnattr_t*,int)=dlsym(RTLD_DEFAULT,"responsibility_spawnattrs_setdisclaim");
 char*av[]={"/bin/ls","-d",v[1],NULL}; char*av2[]={"/bin/ls",v[1],NULL}; pid_t p; int st;
 for(int d=0;d<2;d++){ posix_spawnattr_t a; posix_spawnattr_init(&a); if(d) disc(&a,1);
  printf("disclaim=%d: ",d); fflush(stdout); posix_spawn(&p,"/bin/ls",NULL,&a,av2,environ); waitpid(p,&st,0); printf("  -> exit %d\n",WEXITSTATUS(st)); }
 return 0;}
