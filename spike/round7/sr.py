import ctypes,os
before=open('/proc/self/status').read()
libc=ctypes.CDLL(None); print("prctl set subreaper rc", libc.prctl(36,1,0,0,0))
after=open('/proc/self/status').read()
b=set(before.splitlines()); a=set(after.splitlines())
print("status lines changed:", sorted(a-b))
st1=open('/proc/1/stat').read(); print("pid1 comm/state ok")
