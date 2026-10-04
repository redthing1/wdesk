// Small managed fixture for bitness, literal argv and process-local environment.
using System;
using System.IO;
using System.Text;

class GraphicsEnvironmentProbe {
  static string Quote(string value) {
    if(value==null)return "null";
    var result=new StringBuilder("\"");
    foreach(char c in value) {
      if(c=='"' || c=='\\')result.Append('\\').Append(c);
      else if(c<32)result.Append("\\u").Append(((int)c).ToString("x4"));
      else result.Append(c);
    }
    return result.Append('"').ToString();
  }
  static void Main(string[] argv) {
    Console.OutputEncoding=new UTF8Encoding(false);
    var arguments=new string[argv.Length];
    for(int i=0;i<argv.Length;i++)arguments[i]=Quote(argv[i]);
    Console.WriteLine("{\"architecture\":"+Quote(Environment.Is64BitProcess?"x64":"x86")+
      ",\"driver\":"+Quote(Environment.GetEnvironmentVariable("GALLIUM_DRIVER"))+
      ",\"threads\":"+Quote(Environment.GetEnvironmentVariable("LP_NUM_THREADS"))+
      ",\"vulkan_driver\":"+Quote(Environment.GetEnvironmentVariable("VK_DRIVER_FILES"))+
      ",\"cwd\":"+Quote(Directory.GetCurrentDirectory())+
      ",\"argv\":["+String.Join(",",arguments)+"]}");
  }
}
