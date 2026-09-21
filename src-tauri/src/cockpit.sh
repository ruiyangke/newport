# Read-only Linux collector, streamed through SSH; requires standard Linux tools.
set -eu
[ -r /proc/stat ] && [ -r /proc/meminfo ] || { printf 'System monitoring currently supports Linux servers.\n' >&2; exit 1; }
export LC_ALL=C
awk '
function quote(s,    i,c,out) {
  out="\""
  for(i=1;i<=length(s);i++) {
    c=substr(s,i,1)
    if(c=="\\") out=out "\\\\"
    else if(c=="\"") out=out "\\\""
    else if(c=="\t") out=out "\\t"
    else if(c=="\r") out=out "\\r"
    else if(c ~ /[[:cntrl:]]/) out=out "?"
    else out=out c
  }
  return out "\""
}
function first(cmd,    line) { cmd | getline line; close(cmd); return line }
function cpu(    line,a,i) {
  getline line < "/proc/stat"; close("/proc/stat"); split(line,a," ")
  total=0; for(i=2;i<=9;i++) total+=a[i]
  idle=a[5]+a[6]
}
BEGIN {
  cpu(); oldtotal=total; oldidle=idle; system("sleep 0.3"); cpu()
  usage=(total>oldtotal) ? 100*(1-(idle-oldidle)/(total-oldtotal)) : 0
  if(usage<0) usage=0; if(usage>100) usage=100
  while((getline line < "/proc/meminfo")>0) { split(line,a," "); sub(/:$/,"",a[1]); mem[a[1]]=a[2]*1024 }
  close("/proc/meminfo")
  getline line < "/proc/uptime"; split(line,a," "); up=a[1]+0; close("/proc/uptime")
  getline line < "/proc/loadavg"; split(line,a," "); load=sprintf("[%s,%s,%s]",a[1],a[2],a[3]); close("/proc/loadavg")
  os="Linux"
  while((getline line < "/etc/os-release")>0) if(line ~ /^PRETTY_NAME=/) { sub(/^PRETTY_NAME=/,"",line); sub(/^\"/,"",line); sub(/\"$/,"",line); os=line }
  close("/etc/os-release")
  cores=first("getconf _NPROCESSORS_ONLN 2>/dev/null")+0; if(cores<1) cores=1
  available=("MemAvailable" in mem) ? mem["MemAvailable"] : mem["MemFree"]
  printf "{\"hostname\":%s,\"os\":%s,\"kernel\":%s,\"cores\":%d,\"uptime\":%f,\"load\":%s,\"cpu\":%f,", quote(first("uname -n")),quote(os),quote(first("uname -r")),cores,up,load,usage
  printf "\"memoryTotal\":%.0f,\"memoryUsed\":%.0f,\"swapTotal\":%.0f,\"swapUsed\":%.0f,",mem["MemTotal"],mem["MemTotal"]-available,mem["SwapTotal"],mem["SwapTotal"]-mem["SwapFree"]
  printf "\"disks\":["; sep=""
  cmd="df -P -B1 -l 2>/dev/null"
  while((cmd | getline line)>0) {
    split(line,a," ")
    if(a[2] !~ /^[0-9]+$/ || (a[1] !~ /^\/dev\// && a[6]!="/")) continue
    mount=line; for(i=1;i<=5;i++) sub(/^[[:space:]]*[^[:space:]]+[[:space:]]+/,"",mount)
    printf "%s{\"mount\":%s,\"device\":%s,\"total\":%s,\"used\":%s,\"available\":%s}",sep,quote(mount),quote(a[1]),a[2],a[3],a[4]; sep=","
  }
  close(cmd); printf "],\"network\":["; sep=""
  while((getline line < "/proc/net/dev")>0) {
    if(index(line,":")==0) continue
    split(line,parts,":"); name=parts[1]; gsub(/[[:space:]]/,"",name); split(parts[2],a," ")
    printf "%s{\"name\":%s,\"received\":%s,\"sent\":%s}",sep,quote(name),a[1],a[9]; sep=","
  }
  close("/proc/net/dev"); printf "],\"processes\":["; sep=""; count=0
  cmd="ps -eo pid=,user=,pcpu=,rss=,stat=,comm= --sort=-pcpu 2>/dev/null"
  while((cmd | getline line)>0) {
    count++; if(count>50) continue
    split(line,a," "); name=line; for(i=1;i<=5;i++) sub(/^[[:space:]]*[^[:space:]]+[[:space:]]+/,"",name)
    printf "%s{\"pid\":%d,\"user\":%s,\"cpu\":%f,\"memory\":%.0f,\"state\":%s,\"name\":%s}",sep,a[1],quote(a[2]),a[3],a[4]*1024,quote(a[5]),quote(name); sep=","
  }
  code=close(cmd)
  if(code!=0) { print "Unable to read processes. Install procps on the server." > "/dev/stderr"; exit 1 }
  printf "],\"processCount\":%d,\"processCpuMode\":\"lifetime\"}\n",count
}'
