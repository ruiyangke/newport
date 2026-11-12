// Isolated native acceptance harness. Run only on the selected Windows test VM.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

public sealed class ClipboardStationReceipt
{
    public string Station;
    public string Desktop;
    public string Executable;
    public string Sha256;
    public string Mode;
    public string Test;
    public string StartedUtc;
    public string FinishedUtc;
    public bool Passed;
    public bool OriginalStationRestored;
    public bool StationCreatedFresh;
    public bool StationNonInteractive;
    public bool Brokered;
    public uint SelectedUserSession;
    public uint ExecutionSession;
    public bool SourceTokenUnchanged;
    public ClipboardTokenReceipt SourceToken;
    public ClipboardTokenReceipt ExecutionToken;
    public string Error;
    public readonly List<string> Checkpoints = new List<string>();
    public readonly List<ClipboardProcessReceipt> Processes = new List<ClipboardProcessReceipt>();
    public readonly List<string> CleanupErrors = new List<string>();
}

public sealed class ClipboardProcessReceipt
{
    public string Role;
    public uint Pid;
    public string Command;
    public bool TerminatedByHarness;
    public bool ExitConfirmed;
    public uint ExitCode;
    public bool OutputOverflow;
    public bool AsynchronousOutput;
    public bool OutputDrainConfirmed;
    public bool OutputReadersStopped;
    public bool TokenVerifiedBeforeResume;
    public ClipboardTokenReceipt Token;
}

public sealed class ClipboardTokenReceipt
{
    public string UserSid;
    public string LogonSid;
    public string AuthenticationId;
    public string IntegritySid;
    public string GroupsSha256;
    public string PrivilegesSha256;
    public bool Elevated;
    public bool UIAccess;
    public uint Session;
}

public static class ClipboardStationHarness
{
    const uint WAIT_OBJECT_0 = 0;
    const uint HANDLE_FLAG_INHERIT = 1;
    const uint STARTF_USESTDHANDLES = 0x100;
    const uint CREATE_UNICODE_ENVIRONMENT = 0x400;
    const uint EXTENDED_STARTUPINFO_PRESENT = 0x80000;
    const uint CREATE_NO_WINDOW = 0x8000000;
    const uint CREATE_SUSPENDED = 4;
    const uint CWF_CREATE_ONLY = 1;
    const uint WSF_VISIBLE = 1;
    const uint WINSTA_ALL_ACCESS = 0x37f;
    const uint DESKTOP_ALL_ACCESS = 0x1ff;
    const uint TOKEN_ASSIGN_PRIMARY = 0x1;
    const uint TOKEN_DUPLICATE = 0x2;
    const uint TOKEN_QUERY = 0x8;
    const uint TOKEN_ADJUST_DEFAULT = 0x80;
    const uint TOKEN_ADJUST_SESSIONID = 0x100;
    const uint HARNESS_TERMINATION = 0xee01;
    const int MAX_LOG_BYTES = 1024 * 1024;
    const int MAX_LINE_BYTES = 4096;
    static readonly IntPtr INVALID_HANDLE_VALUE = new IntPtr(-1);
    static readonly string[] Markers = {
        "CLIPBOARD_CHILD_READY", "CLIPBOARD_CHILD_ENTERED", "CLIPBOARD_PARENT_READY",
        "CLIPBOARD_PARENT_STARTED", "CLIPBOARD_PARENT_STOPPED", "CLIPBOARD_READER_RECOVERED"
    };

    [StructLayout(LayoutKind.Sequential)]
    struct SECURITY_ATTRIBUTES { public int Length; public IntPtr Descriptor; public int Inherit; }
    [StructLayout(LayoutKind.Sequential)]
    struct USEROBJECTFLAGS { public int Inherit, Reserved; public uint Flags; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct STARTUPINFO {
        public uint cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public uint dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public ushort wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct STARTUPINFOEX { public STARTUPINFO StartupInfo; public IntPtr Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    struct PROCESS_INFORMATION { public IntPtr Process, Thread; public uint Pid, Tid; }
    [StructLayout(LayoutKind.Sequential)]
    struct LUID { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)]
    struct LUID_AND_ATTRIBUTES { public LUID Luid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    struct TOKEN_PRIVILEGES_ONE { public uint Count; public LUID_AND_ATTRIBUTES Privilege; }
    [StructLayout(LayoutKind.Sequential)]
    struct SID_AND_ATTRIBUTES { public IntPtr Sid; public uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    struct TOKEN_GROUPS_ONE { public uint Count; public SID_AND_ATTRIBUTES Group; }
    [StructLayout(LayoutKind.Sequential)]
    struct JOB_BASIC_LIMITS {
        public long ProcessTime, JobTime; public uint Flags;
        public UIntPtr MinimumWorkingSet, MaximumWorkingSet; public uint ActiveProcesses;
        public UIntPtr Affinity; public uint Priority, Scheduling;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct IO_COUNTERS { public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes; }
    [StructLayout(LayoutKind.Sequential)]
    struct JOB_LIMITS {
        public JOB_BASIC_LIMITS Basic; public IO_COUNTERS Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateWindowStationW(string name, uint flags, uint access, ref SECURITY_ATTRIBUTES security);
    [DllImport("user32.dll", SetLastError = true)]
    static extern bool CloseWindowStation(IntPtr station);
    [DllImport("user32.dll", SetLastError = true)]
    static extern IntPtr GetProcessWindowStation();
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetUserObjectInformationW(IntPtr handle, int information, IntPtr buffer, uint bytes, out uint needed);
    [DllImport("user32.dll", SetLastError = true)]
    static extern bool SetProcessWindowStation(IntPtr station);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr mode, uint flags, uint access, ref SECURITY_ATTRIBUTES security);
    [DllImport("user32.dll", SetLastError = true)]
    static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string text, uint revision, out IntPtr descriptor, out uint size);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr memory);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CreatePipe(out IntPtr read, out IntPtr write, ref SECURITY_ATTRIBUTES security, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    static extern IntPtr CreateNamedPipeW(string name, uint openMode, uint pipeMode, uint maximumInstances, uint outputBuffer, uint inputBuffer, uint defaultTimeout, ref SECURITY_ATTRIBUTES security);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateFileW(string name, uint access, uint share, ref SECURITY_ATTRIBUTES security, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetNamedPipeClientProcessId(IntPtr pipe, out uint pid);
    [DllImport("kernel32.dll")]
    static extern uint GetCurrentProcessId();
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CancelIoEx(SafePipeHandle pipe, IntPtr overlapped);
    [DllImport("kernel32.dll")]
    static extern void SetLastError(uint error);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr attributes, int count, uint flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr attributes, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previous, IntPtr returnedSize);
    [DllImport("kernel32.dll")]
    static extern void DeleteProcThreadAttributeList(IntPtr attributes);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string executable, StringBuilder command, IntPtr processSecurity, IntPtr threadSecurity, bool inheritHandles, uint flags, IntPtr environment, string directory, ref STARTUPINFOEX startup, out PROCESS_INFORMATION process);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessAsUserW(IntPtr token, string executable, StringBuilder command, IntPtr processSecurity, IntPtr threadSecurity, bool inheritHandles, uint flags, IntPtr environment, string directory, ref STARTUPINFOEX startup, out PROCESS_INFORMATION process);
    [DllImport("wtsapi32.dll", SetLastError = true)]
    static extern bool WTSQueryUserToken(uint session, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ProcessIdToSessionId(uint pid, out uint session);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetTokenInformation(IntPtr token, int information, IntPtr buffer, uint length, out uint needed);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool SetTokenInformation(IntPtr token, int information, ref uint value, uint length);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool DuplicateTokenEx(IntPtr token, uint access, IntPtr security, int impersonation, int type, out IntPtr duplicate);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool LookupPrivilegeValueW(string system, string name, out LUID luid);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TOKEN_PRIVILEGES_ONE state, uint length, out TOKEN_PRIVILEGES_ONE previous, out uint needed);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetSecurityDescriptorSacl(IntPtr descriptor, out bool present, out IntPtr sacl, out bool defaulted);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    static extern uint SetNamedSecurityInfoW(string name, int type, uint information, IntPtr owner, IntPtr group, IntPtr dacl, IntPtr sacl);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateJobObjectW(ref SECURITY_ATTRIBUTES security, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr job, int informationClass, ref JOB_LIMITS limits, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr thread);

    static void Require(bool ok, string action) {
        if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error(), action);
    }
    static void Close(ref IntPtr handle) {
        if (handle != IntPtr.Zero && handle != INVALID_HANDLE_VALUE) CloseHandle(handle);
        handle = IntPtr.Zero;
    }

    sealed class TokenBuffer : IDisposable {
        public IntPtr Data;
        public readonly uint Length;
        public TokenBuffer(IntPtr token, int information) {
            uint needed;
            GetTokenInformation(token, information, IntPtr.Zero, 0, out needed);
            if (needed == 0 || needed > 1024 * 1024) throw new Win32Exception(Marshal.GetLastWin32Error(), "Size token information");
            Length = needed; Data = Marshal.AllocHGlobal((int)needed);
            try {
                Require(GetTokenInformation(token, information, Data, Length, out needed), "Read token information");
                if (needed > Length) throw new InvalidOperationException("Token information grew past its allocation");
            } catch { Dispose(); throw; }
        }
        public void RequireBytes(int offset, int length) {
            if (offset < 0 || length < 0 || (long)offset + length > Length) throw new InvalidOperationException("Invalid token information bounds");
        }
        public void Dispose() { if (Data != IntPtr.Zero) Marshal.FreeHGlobal(Data); Data = IntPtr.Zero; }
    }
    static uint TokenNumber(IntPtr token, int information) {
        using (var buffer = new TokenBuffer(token, information)) {
            buffer.RequireBytes(0, 4); return unchecked((uint)Marshal.ReadInt32(buffer.Data));
        }
    }
    static string TokenSid(IntPtr token, int information) {
        using (var buffer = new TokenBuffer(token, information)) {
            buffer.RequireBytes(0, Marshal.SizeOf(typeof(SID_AND_ATTRIBUTES)));
            var entry = (SID_AND_ATTRIBUTES)Marshal.PtrToStructure(buffer.Data, typeof(SID_AND_ATTRIBUTES));
            return new SecurityIdentifier(entry.Sid).Value;
        }
    }
    static string LuidText(LUID luid) { return unchecked((uint)luid.High).ToString("x8") + luid.Low.ToString("x8"); }
    static string Digest(List<string> values) {
        values.Sort(StringComparer.Ordinal);
        using (var hash = SHA256.Create()) return BitConverter.ToString(hash.ComputeHash(Encoding.UTF8.GetBytes(String.Join("\n", values.ToArray())))).Replace("-", "").ToLowerInvariant();
    }
    static string TokenGroups(IntPtr token, bool logon) {
        using (var buffer = new TokenBuffer(token, logon ? 28 : 2)) {
            buffer.RequireBytes(0, 4); int count = Marshal.ReadInt32(buffer.Data);
            int offset = (int)Marshal.OffsetOf(typeof(TOKEN_GROUPS_ONE), "Group");
            int size = Marshal.SizeOf(typeof(SID_AND_ATTRIBUTES));
            if (count < 0 || count > 4096 || (logon && count != 1)) throw new InvalidOperationException("Expected exactly one Windows logon SID");
            buffer.RequireBytes(offset, checked(count * size));
            var values = new List<string>();
            for (int i = 0; i < count; i++) {
                var entry = (SID_AND_ATTRIBUTES)Marshal.PtrToStructure(IntPtr.Add(buffer.Data, offset + i * size), typeof(SID_AND_ATTRIBUTES));
                string sid = new SecurityIdentifier(entry.Sid).Value;
                if (logon) {
                    if ((entry.Attributes & 0xc0000004) != 0xc0000004 || (entry.Attributes & 0x10) != 0 || !sid.StartsWith("S-1-5-5-", StringComparison.Ordinal))
                        throw new InvalidOperationException("The target token lacks one enabled logon SID");
                    return sid;
                }
                values.Add(sid + ":" + entry.Attributes.ToString("x8"));
            }
            return Digest(values);
        }
    }
    static string TokenPrivileges(IntPtr token) {
        using (var buffer = new TokenBuffer(token, 3)) {
            buffer.RequireBytes(0, 4); int count = Marshal.ReadInt32(buffer.Data);
            int offset = (int)Marshal.OffsetOf(typeof(TOKEN_PRIVILEGES_ONE), "Privilege");
            int size = Marshal.SizeOf(typeof(LUID_AND_ATTRIBUTES));
            if (count < 0 || count > 4096) throw new InvalidOperationException("Invalid token privilege count");
            buffer.RequireBytes(offset, checked(count * size));
            var values = new List<string>();
            for (int i = 0; i < count; i++) {
                var entry = (LUID_AND_ATTRIBUTES)Marshal.PtrToStructure(IntPtr.Add(buffer.Data, offset + i * size), typeof(LUID_AND_ATTRIBUTES));
                values.Add(LuidText(entry.Luid) + ":" + entry.Attributes.ToString("x8"));
            }
            return Digest(values);
        }
    }
    static ClipboardTokenReceipt ReadOrdinaryToken(IntPtr token, string expectedSid, uint expectedSession) {
        var result = new ClipboardTokenReceipt {
            UserSid = TokenSid(token, 1), LogonSid = TokenGroups(token, true), IntegritySid = TokenSid(token, 25),
            GroupsSha256 = TokenGroups(token, false), PrivilegesSha256 = TokenPrivileges(token),
            Elevated = TokenNumber(token, 20) != 0, UIAccess = TokenNumber(token, 26) != 0, Session = TokenNumber(token, 12)
        };
        using (var buffer = new TokenBuffer(token, 10)) {
            buffer.RequireBytes(8, Marshal.SizeOf(typeof(LUID)));
            result.AuthenticationId = LuidText((LUID)Marshal.PtrToStructure(IntPtr.Add(buffer.Data, 8), typeof(LUID)));
        }
        if (result.UserSid != expectedSid || result.UserSid == "S-1-5-18" || result.Session != expectedSession ||
            result.Elevated || result.UIAccess || result.IntegritySid != "S-1-16-8192" || TokenNumber(token, 8) != 1)
            throw new InvalidOperationException("The selected token must be the expected non-elevated, medium-integrity ordinary user primary token");
        return result;
    }
    static void RequireSameIdentity(ClipboardTokenReceipt expected, ClipboardTokenReceipt actual) {
        if (expected.UserSid != actual.UserSid || expected.LogonSid != actual.LogonSid || expected.AuthenticationId != actual.AuthenticationId ||
            expected.IntegritySid != actual.IntegritySid || expected.Elevated != actual.Elevated || expected.UIAccess != actual.UIAccess ||
            expected.GroupsSha256 != actual.GroupsSha256 || expected.PrivilegesSha256 != actual.PrivilegesSha256)
            throw new InvalidOperationException("Duplicating the user token changed its identity, groups, integrity or privileges");
    }
    sealed class TcbPrivilege : IDisposable {
        IntPtr token;
        TOKEN_PRIVILEGES_ONE previous;
        bool changed;
        public TcbPrivilege() {
            try {
                Require(OpenProcessToken(GetCurrentProcess(), 0x20 | 0x8, out token), "Open broker privilege token");
                LUID luid; Require(LookupPrivilegeValueW(null, "SeTcbPrivilege", out luid), "Find broker token privilege");
                var state = new TOKEN_PRIVILEGES_ONE { Count = 1, Privilege = new LUID_AND_ATTRIBUTES { Luid = luid, Attributes = 2 } };
                uint needed;
                SetLastError(0);
                Require(AdjustTokenPrivileges(token, false, ref state, (uint)Marshal.SizeOf(typeof(TOKEN_PRIVILEGES_ONE)), out previous, out needed), "Enable broker token privilege");
                int error = Marshal.GetLastWin32Error();
                changed = previous.Count != 0;
                if (error != 0) throw new Win32Exception(error, "Broker does not hold the required token privilege");
            } catch { Dispose(); throw; }
        }
        public void Dispose() {
            try {
                if (changed) {
                    TOKEN_PRIVILEGES_ONE ignored; uint needed;
                    SetLastError(0);
                    Require(AdjustTokenPrivileges(token, false, ref previous, (uint)Marshal.SizeOf(typeof(TOKEN_PRIVILEGES_ONE)), out ignored, out needed), "Restore broker token privilege");
                    int error = Marshal.GetLastWin32Error();
                    if (error != 0) throw new Win32Exception(error, "Restore broker token privilege");
                    changed = false;
                }
            } finally { Close(ref token); }
        }
    }
    sealed class UserToken : IDisposable {
        IntPtr source;
        public IntPtr Primary;
        public readonly ClipboardTokenReceipt Original;
        readonly uint selectedSession;
        readonly uint executionSession;
        public UserToken(uint selected, string expectedSid, uint execution) {
            selectedSession = selected; executionSession = execution;
            try {
                if (!WindowsIdentity.GetCurrent().IsSystem || execution != 0 || selected == 0)
                    throw new InvalidOperationException("Broker mode requires SYSTEM in session zero and an explicit existing user session");
                expectedSid = new SecurityIdentifier(expectedSid).Value;
                using (var privilege = new TcbPrivilege()) {
                    Require(WTSQueryUserToken(selected, out source), "Query explicitly selected logged-on user token");
                    Original = ReadOrdinaryToken(source, expectedSid, selected);
                    // Session updates require default-information access as well as session access.
                    uint access = TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
                    Require(DuplicateTokenEx(source, access, IntPtr.Zero, 2, 1, out Primary), "Duplicate ordinary user primary token");
                    RequireSameIdentity(Original, ReadOrdinaryToken(Primary, expectedSid, selected));
                    Require(SetTokenInformation(Primary, 12, ref execution, 4), "Select isolated execution session on duplicate token");
                    RequireSameIdentity(Original, ReadOrdinaryToken(Primary, expectedSid, execution));
                    VerifySource();
                }
            } catch { Dispose(); throw; }
        }
        public ClipboardTokenReceipt Execution { get { return ReadOrdinaryToken(Primary, Original.UserSid, executionSession); } }
        public void VerifySource() { RequireSameIdentity(Original, ReadOrdinaryToken(source, Original.UserSid, selectedSession)); }
        public ClipboardTokenReceipt VerifyChild(IntPtr process) {
            IntPtr token = IntPtr.Zero;
            try {
                Require(OpenProcessToken(process, 0x8, out token), "Read suspended fixture token");
                var actual = ReadOrdinaryToken(token, Original.UserSid, executionSession);
                RequireSameIdentity(Original, actual); VerifySource(); return actual;
            } finally { Close(ref token); }
        }
        public void Dispose() { Close(ref Primary); Close(ref source); }
    }
    static void GrantFixtureDirectory(string directory, string logonSid) {
        var info = new DirectoryInfo(directory);
        var security = new DirectorySecurity();
        security.SetAccessRuleProtection(true, false);
        foreach (string sid in new [] { "S-1-5-18", logonSid }) {
            security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid), FileSystemRights.FullControl,
                InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
        }
        info.SetAccessControl(security);
        IntPtr descriptor = IntPtr.Zero;
        try {
            uint size; Require(ConvertStringSecurityDescriptorToSecurityDescriptorW("S:(ML;OICI;NW;;;ME)", 1, out descriptor, out size), "Create fixture directory integrity label");
            bool present, defaulted; IntPtr sacl;
            Require(GetSecurityDescriptorSacl(descriptor, out present, out sacl, out defaulted), "Read fixture integrity label");
            if (!present || sacl == IntPtr.Zero) throw new InvalidOperationException("Missing fixture integrity label");
            uint error = SetNamedSecurityInfoW(directory, 1, 0x10, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, sacl);
            if (error != 0) throw new Win32Exception((int)error, "Set disposable fixture directory integrity label");
        } finally { if (descriptor != IntPtr.Zero) LocalFree(descriptor); }
    }

    static string ObjectName(IntPtr handle) {
        uint needed;
        GetUserObjectInformationW(handle, 2, IntPtr.Zero, 0, out needed);
        if (needed < 2 || needed > 4096 || needed % 2 != 0) throw new InvalidOperationException("Invalid native object name size");
        IntPtr buffer = Marshal.AllocHGlobal((int)needed);
        try {
            uint capacity = needed;
            Require(GetUserObjectInformationW(handle, 2, buffer, capacity, out needed), "Read native object identity");
            string name = Marshal.PtrToStringUni(buffer, (int)capacity / 2).TrimEnd('\0');
            if (String.IsNullOrEmpty(name) || name.IndexOf('\0') >= 0) throw new InvalidOperationException("Invalid native object name");
            return name;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    static void RequireNonInteractiveStation(IntPtr station, string name) {
        if (String.Equals(name, "WinSta0", StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Interactive clipboard is forbidden");
        uint size = (uint)Marshal.SizeOf(typeof(USEROBJECTFLAGS));
        IntPtr buffer = Marshal.AllocHGlobal((int)size);
        try {
            uint needed;
            Require(GetUserObjectInformationW(station, 1, buffer, size, out needed), "Read isolated station flags");
            var flags = (USEROBJECTFLAGS)Marshal.PtrToStructure(buffer, typeof(USEROBJECTFLAGS));
            if ((flags.Flags & WSF_VISIBLE) != 0) throw new InvalidOperationException("Visible clipboard station is forbidden");
        } finally { Marshal.FreeHGlobal(buffer); }
    }

    static NamedPipeServerStream OutputPipe(IntPtr descriptor, out IntPtr writer) {
        writer = IntPtr.Zero;
        IntPtr server = IntPtr.Zero;
        SafePipeHandle owned = null;
        try {
            var security = new SECURITY_ATTRIBUTES { Length = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)), Descriptor = descriptor, Inherit = 0 };
            string name = "\\\\.\\pipe\\NewportClipboardTest-" + Guid.NewGuid().ToString("N");
            // One local-only instance; the reader is genuinely overlapped and never inherited.
            server = CreateNamedPipeW(name, 1 | 0x40000000 | 0x80000, 8, 1, 4096, 4096, 0, ref security);
            Require(server != INVALID_HANDLE_VALUE, "Create private overlapped output pipe");
            security.Inherit = 1;
            writer = CreateFileW(name, 0x40000000, 0, ref security, 3, 0, IntPtr.Zero);
            Require(writer != INVALID_HANDLE_VALUE, "Connect harness-owned output writer");
            // CreateFile connects before ConnectNamedPipe. Verify that no other process won
            // that race before marking the managed server connected or launching a fixture.
            uint client;
            Require(GetNamedPipeClientProcessId(server, out client), "Identify output pipe client");
            if (client != GetCurrentProcessId()) throw new InvalidOperationException("Output pipe connected to a foreign process");
            owned = new SafePipeHandle(server, true); server = IntPtr.Zero;
            var stream = new NamedPipeServerStream(PipeDirection.In, true, true, owned);
            owned = null; return stream;
        } catch {
            Close(ref writer); throw;
        } finally {
            if (owned != null) owned.Dispose();
            Close(ref server);
        }
    }
    static StreamWriter InputWriter(ref IntPtr handle) {
        SafeFileHandle owned = new SafeFileHandle(handle, true); handle = IntPtr.Zero;
        FileStream stream = null;
        try {
            stream = new FileStream(owned, FileAccess.Write, 4096, false); owned = null;
            var writer = new StreamWriter(stream, new UTF8Encoding(false)); stream = null;
            writer.NewLine = "\n"; writer.AutoFlush = true; return writer;
        } finally {
            if (stream != null) stream.Dispose();
            if (owned != null) owned.Dispose();
        }
    }
    static string Quote(string value) {
        var result = new StringBuilder("\""); int slashes = 0;
        foreach (char c in value) {
            if (c == '\\') { slashes++; continue; }
            if (c == '"') { result.Append('\\', slashes * 2 + 1); result.Append(c); }
            else { result.Append('\\', slashes); result.Append(c); }
            slashes = 0;
        }
        result.Append('\\', slashes * 2); result.Append('"'); return result.ToString();
    }
    static string EnvironmentBlock(string station, string desktop, string mode, string output) {
        // Do not inherit credentials, account integration or a real Newport profile.
        string system = Environment.GetEnvironmentVariable("SystemRoot");
        if (String.IsNullOrEmpty(system) || !Directory.Exists(system)) throw new InvalidOperationException("SystemRoot is unavailable");
        string temporary = Path.Combine(output, "temp"); Directory.CreateDirectory(temporary);
        string profile = Path.Combine(output, "profile"); Directory.CreateDirectory(profile);
        var values = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase) {
            { "SystemRoot", system }, { "WINDIR", system },
            { "PATH", Path.Combine(system, "System32") + ";" + system },
            { "TEMP", temporary }, { "TMP", temporary },
            { "NEWPORT_DATA_DIR", profile }, { "RUST_BACKTRACE", "1" },
            { "NEWPORT_CLIPBOARD_TEST_BASE", Path.Combine(temporary, "clipboard-cache") },
            { "NEWPORT_CLIPBOARD_TEST_STATION", station },
            { "NEWPORT_CLIPBOARD_TEST_STATION_OWNERSHIP", "fresh-create-only" },
            { "NEWPORT_CLIPBOARD_TEST_DESKTOP", desktop },
            { "NEWPORT_CLIPBOARD_PROBE_MODE", mode }
        };
        var result = new StringBuilder();
        foreach (var pair in values) result.Append(pair.Key).Append('=').Append(pair.Value).Append('\0');
        result.Append('\0'); return result.ToString();
    }

    sealed class Pump : IDisposable {
        // A timed-out cancellation must not dispose handles/buffers still used by native I/O.
        // Retain the entire operation until its managed completion; the receipt already fails.
        static readonly ConcurrentDictionary<Pump, byte> Pending = new ConcurrentDictionary<Pump, byte>();
        readonly NamedPipeServerStream input;
        readonly FileStream log;
        readonly Task task;
        readonly CancellationTokenSource cancellation = new CancellationTokenSource();
        public readonly ConcurrentDictionary<string, bool> Seen = new ConcurrentDictionary<string, bool>();
        public volatile bool Overflow;
        public volatile string Error;
        int disposalRequested;
        int resourcesReleased;
        public Pump(NamedPipeServerStream stream, string logPath) {
            input = stream;
            log = new FileStream(logPath, FileMode.CreateNew, FileAccess.Write, FileShare.Read);
            task = Read();
        }
        async Task Read() {
            try {
                var buffer = new byte[1024]; int total = 0;
                using (var line = new MemoryStream()) {
                    while (true) {
                        int count = await input.ReadAsync(buffer, 0, buffer.Length, cancellation.Token).ConfigureAwait(false);
                        if (count == 0) break;
                        if (count > MAX_LOG_BYTES - total) { Overflow = true; }
                        else { log.Write(buffer, 0, count); log.Flush(); total += count; }
                        for (int i = 0; i < count; i++) {
                            if (buffer[i] == 10) {
                                string text = Encoding.UTF8.GetString(line.ToArray()).TrimEnd('\r'); line.SetLength(0);
                                foreach (string marker in Markers) if (text.EndsWith(marker, StringComparison.Ordinal)) Seen.TryAdd(marker, true);
                                if (text.Contains("test result: ok. 1 passed; 0 failed; 0 ignored;")) Seen.TryAdd("LIBTEST_ONE_PASSED", true);
                            } else if (line.Length < MAX_LINE_BYTES) line.WriteByte(buffer[i]);
                            else Overflow = true;
                        }
                    }
                }
            } catch (OperationCanceledException) {
                if (!cancellation.IsCancellationRequested) Error = "Unexpected pipe read cancellation";
            } catch (IOException error) {
                if (!cancellation.IsCancellationRequested || (error.HResult & 0xffff) != 995)
                    Error = error.GetType().Name + ": " + error.Message;
            } catch (Exception error) { Error = error.GetType().Name + ": " + error.Message; }
        }
        public bool Finish() { return task.Wait(2000); }
        public bool Stopped { get { return task.IsCompleted; } }
        void ReleaseResources() {
            if (Interlocked.Exchange(ref resourcesReleased, 1) != 0) return;
            try { input.Dispose(); }
            finally { try { log.Dispose(); } finally { cancellation.Dispose(); } }
        }
        public void Dispose() {
            if (Interlocked.Exchange(ref disposalRequested, 1) != 0) return;
            Exception cancellationError = null;
            if (!task.IsCompleted) {
                try {
                    cancellation.Cancel();
                    // Also cancel native operations on frameworks whose ReadAsync only checks
                    // the token before submission. SafePipeHandle stays owned until completion.
                    if (!CancelIoEx(input.SafePipeHandle, IntPtr.Zero)) {
                        int error = Marshal.GetLastWin32Error();
                        if (error != 1168) throw new Win32Exception(error, "Cancel owned overlapped pipe read");
                    }
                } catch (Exception error) { cancellationError = error; }
            }
            if (!task.Wait(2000)) {
                Pending.TryAdd(this, 0);
                task.ContinueWith(delegate(Task completed) {
                    try { ReleaseResources(); }
                    catch (Exception error) { Error = error.GetType().Name + ": " + error.Message; }
                    finally { byte ignored; Pending.TryRemove(this, out ignored); }
                }, TaskScheduler.Default);
                throw new TimeoutException("Owned asynchronous pipe reader did not stop; resources retained until I/O completion");
            }
            ReleaseResources();
            if (cancellationError != null) throw cancellationError;
        }
    }

    sealed class Child : IDisposable {
        IntPtr process;
        readonly Pump output;
        readonly Pump errors;
        readonly StreamWriter input;
        readonly List<string> cleanupErrors;
        int nextMessage;
        static readonly string[] InputMessages = { "ready", "blocked", "terminated" };
        public readonly ClipboardProcessReceipt Receipt;
        public Child(string executable, string test, string role, string desktop, string environment, string directory, IntPtr job, IntPtr descriptor, UserToken user, ClipboardStationReceipt run) {
            cleanupErrors = run.CleanupErrors;
            IntPtr stdinRead = IntPtr.Zero, stdinWrite = IntPtr.Zero, stdoutWrite = IntPtr.Zero, stderrWrite = IntPtr.Zero;
            NamedPipeServerStream stdoutRead = null, stderrRead = null;
            IntPtr attributes = IntPtr.Zero, handles = IntPtr.Zero, env = IntPtr.Zero;
            bool attributesReady = false;
            var pi = new PROCESS_INFORMATION();
            try {
                var security = new SECURITY_ATTRIBUTES { Length = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)), Descriptor = descriptor, Inherit = 1 };
                Require(CreatePipe(out stdinRead, out stdinWrite, ref security, 4096), "Create stdin pipe");
                stdoutRead = OutputPipe(descriptor, out stdoutWrite);
                stderrRead = OutputPipe(descriptor, out stderrWrite);
                Require(SetHandleInformation(stdinWrite, HANDLE_FLAG_INHERIT, 0), "Protect parent stdin handle");
                IntPtr size = IntPtr.Zero;
                InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
                if (size == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Size handle allowlist");
                attributes = Marshal.AllocHGlobal(size);
                Require(InitializeProcThreadAttributeList(attributes, 1, 0, ref size), "Initialize handle allowlist"); attributesReady = true;
                handles = Marshal.AllocHGlobal(IntPtr.Size * 3);
                Marshal.WriteIntPtr(handles, 0, stdinRead); Marshal.WriteIntPtr(handles, IntPtr.Size, stdoutWrite); Marshal.WriteIntPtr(handles, IntPtr.Size * 2, stderrWrite);
                Require(UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x20002), handles, new IntPtr(IntPtr.Size * 3), IntPtr.Zero, IntPtr.Zero), "Set inherited handle allowlist");
                var startup = new STARTUPINFOEX {
                    StartupInfo = new STARTUPINFO { cb = (uint)Marshal.SizeOf(typeof(STARTUPINFOEX)), lpDesktop = desktop, dwFlags = STARTF_USESTDHANDLES, hStdInput = stdinRead, hStdOutput = stdoutWrite, hStdError = stderrWrite },
                    Attributes = attributes
                };
                string command = Quote(executable) + " " + Quote(test) + " --exact --ignored --nocapture --test-threads=1";
                env = Marshal.StringToHGlobalUni(environment);
                uint flags = CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW;
                bool created = user == null
                    ? CreateProcessW(executable, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero, true, flags, env, directory, ref startup, out pi)
                    : CreateProcessAsUserW(user.Primary, executable, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero, true, flags, env, directory, ref startup, out pi);
                Require(created, "Create owned fixture process");
                process = pi.Process; pi.Process = IntPtr.Zero;
                Receipt = new ClipboardProcessReceipt { Role = role, Pid = pi.Pid, Command = command };
                run.Processes.Add(Receipt);
                Require(AssignProcessToJobObject(job, process), "Confine fixture to owned kill-on-close job");
                if (user != null) {
                    Receipt.Token = user.VerifyChild(process);
                    Receipt.TokenVerifiedBeforeResume = true;
                }
                Close(ref stdinRead); Close(ref stdoutWrite); Close(ref stderrWrite);
                input = InputWriter(ref stdinWrite);
                output = new Pump(stdoutRead, Path.Combine(directory, role + ".stdout.log")); stdoutRead = null;
                errors = new Pump(stderrRead, Path.Combine(directory, role + ".stderr.log")); stderrRead = null;
                Receipt.AsynchronousOutput = true;
                Require(ResumeThread(pi.Thread) != 0xffffffff, "Start confined fixture process");
                Close(ref pi.Thread);
            } catch {
                // Drop our duplicate writers and kill the confined processes before draining.
                // This also handles a constructor failing after another fixture has started.
                Close(ref stdinRead); Close(ref stdoutWrite); Close(ref stderrWrite);
                foreach (var entry in run.Processes) if (!entry.ExitConfirmed) entry.TerminatedByHarness = true;
                if (!TerminateJobObject(job, HARNESS_TERMINATION)) cleanupErrors.Add(role + ": terminate job after launch failure: " + Marshal.GetLastWin32Error());
                if (process != IntPtr.Zero) {
                    try { Terminate(); } catch (Exception error) { cleanupErrors.Add(role + ": " + error.Message); }
                    Close(ref process);
                }
                if (input != null) try { input.Dispose(); } catch (Exception error) { cleanupErrors.Add(role + ": " + error.Message); }
                if (output != null) try { output.Dispose(); } catch (Exception error) { cleanupErrors.Add(role + ": " + error.Message); }
                if (errors != null) try { errors.Dispose(); } catch (Exception error) { cleanupErrors.Add(role + ": " + error.Message); }
                if (Receipt != null) Receipt.OutputReadersStopped = (output == null || output.Stopped) && (errors == null || errors.Stopped);
                throw;
            } finally {
                Close(ref stdinRead); Close(ref stdinWrite); Close(ref stdoutWrite); Close(ref stderrWrite);
                if (stdoutRead != null) stdoutRead.Dispose(); if (stderrRead != null) stderrRead.Dispose();
                Close(ref pi.Process); Close(ref pi.Thread);
                if (attributesReady) DeleteProcThreadAttributeList(attributes);
                if (attributes != IntPtr.Zero) Marshal.FreeHGlobal(attributes);
                if (handles != IntPtr.Zero) Marshal.FreeHGlobal(handles);
                if (env != IntPtr.Zero) Marshal.FreeHGlobal(env);
            }
        }
        public bool Exited { get { return WaitForSingleObject(process, 0) == WAIT_OBJECT_0; } }
        public bool Seen(string marker) { return output.Seen.ContainsKey(marker); }
        public void CheckOutput() {
            if (output.Overflow || errors.Overflow) { Receipt.OutputOverflow = true; throw new InvalidOperationException("Fixture output exceeded its bound"); }
            if (output.Error != null || errors.Error != null) throw new IOException(output.Error ?? errors.Error);
        }
        public void Send(string value) {
            // At most 25 bytes in a 4096-byte anonymous pipe, even if the fixture stops
            // consuming input. There is no arbitrary input or retry that can fill this pipe.
            if (nextMessage >= InputMessages.Length || value != InputMessages[nextMessage]) throw new InvalidOperationException("Invalid fixture stdin checkpoint");
            CheckOutput(); nextMessage++; input.WriteLine(value);
        }
        public void MarkJobTermination() { if (!Exited) Receipt.TerminatedByHarness = true; }
        public void Terminate() {
            if (!Exited) {
                if (TerminateProcess(process, HARNESS_TERMINATION)) Receipt.TerminatedByHarness = true;
                else {
                    int error = Marshal.GetLastWin32Error();
                    // Job termination may already be completing. An exact confirmed exit
                    // is sufficient; do not turn that expected race into a cleanup failure.
                    if (WaitForSingleObject(process, 5000) != WAIT_OBJECT_0)
                        throw new Win32Exception(error, "Terminate exact owned fixture process");
                }
            }
            ConfirmExit(5000);
        }
        public void ConfirmExit(uint milliseconds) {
            uint waited = WaitForSingleObject(process, milliseconds);
            if (waited != WAIT_OBJECT_0) throw new InvalidOperationException("Owned fixture exit was not confirmed; wait result " + waited);
            uint code; Require(GetExitCodeProcess(process, out code), "Read fixture exit status");
            Receipt.ExitConfirmed = true; Receipt.ExitCode = code;
        }
        public void CompleteOutput() {
            bool outputFinished = output.Finish(); bool errorsFinished = errors.Finish();
            if (!outputFinished || !errorsFinished) throw new TimeoutException("Fixture pipe drain did not finish after exit");
            CheckOutput();
            Receipt.OutputDrainConfirmed = true;
        }
        public void Dispose() {
            try { Terminate(); } catch (Exception error) { cleanupErrors.Add(Receipt.Role + ": " + error.Message); }
            try { input.Dispose(); } catch (Exception error) { cleanupErrors.Add(Receipt.Role + ": " + error.Message); }
            try { CompleteOutput(); } catch (Exception error) { cleanupErrors.Add(Receipt.Role + ": " + error.Message); }
            try { output.Dispose(); } catch (Exception error) { cleanupErrors.Add(Receipt.Role + ": " + error.Message); }
            try { errors.Dispose(); } catch (Exception error) { cleanupErrors.Add(Receipt.Role + ": " + error.Message); }
            Receipt.OutputReadersStopped = output.Stopped && errors.Stopped;
            Close(ref process);
        }
    }

    static void WaitMarker(Child child, string marker, Stopwatch deadline, int seconds, ClipboardStationReceipt receipt) {
        while (!child.Seen(marker)) {
            child.CheckOutput();
            if (child.Exited) {
                // Process exit can win a race with the asynchronous final stdout completion.
                child.ConfirmExit(0); child.CompleteOutput();
                if (child.Seen(marker)) break;
                throw new InvalidOperationException(child.Receipt.Role + " exited before " + marker);
            }
            if (deadline.Elapsed.TotalSeconds >= seconds) throw new TimeoutException("Timed out before " + marker);
            Thread.Sleep(10);
        }
        receipt.Checkpoints.Add(marker);
    }

    const string Prefix = "provider::windows::tests::";
    static bool StaticAllowed(string name) {
        return name == "isolated_native_owner_formats_privacy_and_shutdown" || name == "a_copy_before_publication_acknowledgment_keeps_local_origin" || name == "unsupported_file_publication_preserves_the_current_clipboard" ||
            name == "prepared_files_publish_hdrop_copy_effect_and_survive_paste_grace" || name == "canceled_file_preparation_never_empties_the_native_clipboard" || name == "partial_file_commit_keeps_imported_provenance_and_retained_files";
    }

    public static ClipboardStationReceipt Run(string executable, string expectedHash, string mode, string staticTest, string directory, int seconds) {
        return RunInner(executable, expectedHash, mode, staticTest, directory, seconds, null, null);
    }
    public static ClipboardStationReceipt RunAsUser(string executable, string expectedHash, string mode, string staticTest, string directory, int seconds, uint selectedSession, string expectedUserSid) {
        return RunInner(executable, expectedHash, mode, staticTest, directory, seconds, selectedSession, expectedUserSid);
    }
    static ClipboardStationReceipt RunInner(string executable, string expectedHash, string mode, string staticTest, string directory, int seconds, uint? selectedSession, string expectedUserSid) {
        var receipt = new ClipboardStationReceipt { Executable = executable, Mode = mode, Test = staticTest, StartedUtc = DateTime.UtcNow.ToString("o") };
        IntPtr station = IntPtr.Zero, desktop = IntPtr.Zero, descriptor = IntPtr.Zero, job = IntPtr.Zero;
        IntPtr originalStation = IntPtr.Zero;
        bool stationRestoreNeeded = false;
        Child child = null, parent = null;
        FileStream executableLease = null;
        UserToken user = null;
        try {
            if (mode != "static" && mode != "render" && mode != "privacy" && mode != "destroy") throw new ArgumentException("Unknown mode");
            if (mode == "static" && !StaticAllowed(staticTest)) throw new ArgumentException("Static fixture is not allowlisted");
            if (seconds < 15 || seconds > 120) throw new ArgumentException("Timeout must be between 15 and 120 seconds");
            if (!Path.IsPathRooted(directory) || !Directory.Exists(directory) ||
                (File.GetAttributes(directory) & FileAttributes.ReparsePoint) != 0 || Directory.GetFileSystemEntries(directory).Length != 0)
                throw new ArgumentException("The harness requires a fresh empty ordinary output directory");
            if (!Path.IsPathRooted(executable) || !File.Exists(executable)) throw new ArgumentException("An explicit existing test executable is required");
            executable = Path.GetFullPath(executable); receipt.Executable = executable;
            executableLease = new FileStream(executable, FileMode.Open, FileAccess.Read, FileShare.Read);
            using (var sha = SHA256.Create()) receipt.Sha256 = BitConverter.ToString(sha.ComputeHash(executableLease)).Replace("-", "").ToLowerInvariant();
            if (!String.Equals(receipt.Sha256, expectedHash, StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Test executable hash does not match candidate receipt");
            Require(ProcessIdToSessionId(GetCurrentProcessId(), out receipt.ExecutionSession), "Read harness execution session");
            string sid = WindowsIdentity.GetCurrent().User.Value;
            if (selectedSession.HasValue) {
                receipt.Brokered = true; receipt.SelectedUserSession = selectedSession.Value;
                user = new UserToken(selectedSession.Value, expectedUserSid, receipt.ExecutionSession);
                receipt.SourceToken = user.Original; receipt.ExecutionToken = user.Execution;
                receipt.SourceTokenUnchanged = true;
                sid = user.Original.LogonSid;
                GrantFixtureDirectory(directory, sid);
            }
            uint size;
            string label = user == null ? "" : "S:(ML;;NW;;;ME)";
            Require(ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;GA;;;" + sid + ")(A;;GA;;;SY)" + label, 1, out descriptor, out size), "Create selected-token station security");
            var security = new SECURITY_ATTRIBUTES { Length = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES)), Descriptor = descriptor, Inherit = 0 };
            job = CreateJobObjectW(ref security, null); Require(job != IntPtr.Zero, "Create owned fixture job");
            var limits = new JOB_LIMITS { Basic = new JOB_BASIC_LIMITS { Flags = 0x2000 } };
            Require(SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf(typeof(JOB_LIMITS))), "Set kill-on-close fixture job policy");
            // Only administrator tokens may choose a station name. Other tokens use
            // the OS logon-derived name, but must never attach to a station that exists.
            bool canNameStation = user != null || new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
            string requestedStation = canNameStation ? "NewportClipboardTest-" + Guid.NewGuid().ToString("N") : null;
            receipt.Desktop = "ClipboardFixture-" + Guid.NewGuid().ToString("N");
            // Capture the borrowed original before creation can change process association.
            originalStation = GetProcessWindowStation(); Require(originalStation != IntPtr.Zero, "Read original process window station");
            stationRestoreNeeded = true;
            try {
                SetLastError(0);
                station = CreateWindowStationW(requestedStation, CWF_CREATE_ONLY, WINSTA_ALL_ACCESS, ref security);
                int creationError = Marshal.GetLastWin32Error();
                if (station == IntPtr.Zero) throw new Win32Exception(creationError, "Create fresh isolated station; an existing logon-derived station cannot be reused");
                if (creationError == 183) throw new InvalidOperationException("Window station unexpectedly already existed");
                receipt.StationCreatedFresh = true;
                receipt.Station = ObjectName(station);
                if (String.Equals(receipt.Station, ObjectName(originalStation), StringComparison.OrdinalIgnoreCase)) throw new InvalidOperationException("Fixture station must differ from the original station");
                RequireNonInteractiveStation(station, receipt.Station);
                receipt.StationNonInteractive = true;
                Require(SetProcessWindowStation(station), "Select isolated station for desktop creation");
                desktop = CreateDesktopW(receipt.Desktop, IntPtr.Zero, IntPtr.Zero, 0, DESKTOP_ALL_ACCESS, ref security); Require(desktop != IntPtr.Zero, "Create isolated desktop");
            } finally {
                Require(SetProcessWindowStation(originalStation), "Restore harness process window station");
                stationRestoreNeeded = false; receipt.OriginalStationRestored = true;
            }
            string selectedDesktop = receipt.Station + "\\" + receipt.Desktop;
            string environment = EnvironmentBlock(receipt.Station, receipt.Desktop, mode, directory);
            var deadline = Stopwatch.StartNew();
            if (mode != "static") {
                child = new Child(executable, Prefix + "stalled_foreign_owner_process", "child", selectedDesktop, environment, directory, job, descriptor, user, receipt);
                WaitMarker(child, "CLIPBOARD_CHILD_READY", deadline, seconds, receipt);
            }
            parent = new Child(executable, Prefix + (mode == "static" ? staticTest : "cross_process_stalled_owner_does_not_block_shutdown"), "parent", selectedDesktop, environment, directory, job, descriptor, user, receipt);
            if (mode != "static") {
                WaitMarker(parent, "CLIPBOARD_PARENT_READY", deadline, seconds, receipt); parent.Send("ready");
                WaitMarker(parent, "CLIPBOARD_PARENT_STARTED", deadline, seconds, receipt);
                WaitMarker(child, "CLIPBOARD_CHILD_ENTERED", deadline, seconds, receipt); parent.Send("blocked");
                WaitMarker(parent, "CLIPBOARD_PARENT_STOPPED", deadline, seconds, receipt);
                if (child.Exited) throw new InvalidOperationException("Foreign owner exited before the shutdown assertion");
                child.Terminate(); parent.Send("terminated");
                WaitMarker(parent, "CLIPBOARD_READER_RECOVERED", deadline, seconds, receipt);
            }
            while (!parent.Exited) {
                parent.CheckOutput(); if (deadline.Elapsed.TotalSeconds >= seconds) throw new TimeoutException("Parent fixture did not finish within its budget"); Thread.Sleep(10);
            }
            parent.ConfirmExit(0); parent.CompleteOutput();
            if (parent.Receipt.ExitCode != 0) throw new InvalidOperationException("Parent fixture failed with exit code " + parent.Receipt.ExitCode);
            if (!parent.Seen("LIBTEST_ONE_PASSED")) throw new InvalidOperationException("Expected exactly one passing native fixture; a missing test filter is not a pass");
            receipt.Passed = true;
        } catch (Exception error) {
            receipt.Error = error.GetType().Name + ": " + error.Message;
            var native = error as System.ComponentModel.Win32Exception;
            if (native != null) receipt.Error += " (Win32 " + native.NativeErrorCode + ")";
        }
        finally {
            // Close the noninherited kill-on-close job first. No confined descendant can keep
            // an output writer alive while we confirm exact exits and drain/cancel readers.
            if (parent != null) parent.MarkJobTermination(); if (child != null) child.MarkJobTermination();
            if (job != IntPtr.Zero && !CloseHandle(job)) receipt.CleanupErrors.Add("Close fixture job failed: " + Marshal.GetLastWin32Error());
            else job = IntPtr.Zero;
            if (parent != null) parent.Dispose(); if (child != null) child.Dispose();
            Close(ref job);
            if (stationRestoreNeeded) {
                if (!SetProcessWindowStation(originalStation)) receipt.CleanupErrors.Add("Restore original station failed: " + Marshal.GetLastWin32Error());
                else receipt.OriginalStationRestored = true;
            }
            if (desktop != IntPtr.Zero && !CloseDesktop(desktop)) receipt.CleanupErrors.Add("CloseDesktop failed: " + Marshal.GetLastWin32Error());
            if (station != IntPtr.Zero && !CloseWindowStation(station)) receipt.CleanupErrors.Add("CloseWindowStation failed: " + Marshal.GetLastWin32Error());
            if (descriptor != IntPtr.Zero) LocalFree(descriptor);
            if (executableLease != null) executableLease.Dispose();
            if (user != null) {
                try { user.VerifySource(); }
                catch (Exception error) { receipt.SourceTokenUnchanged = false; receipt.CleanupErrors.Add("Original user token verification: " + error.Message); }
                finally { user.Dispose(); }
            }
            if (receipt.CleanupErrors.Count != 0) receipt.Passed = false;
            receipt.FinishedUtc = DateTime.UtcNow.ToString("o");
        }
        return receipt;
    }
}
