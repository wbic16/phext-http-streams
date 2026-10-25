using System.Text;
using System.Text.Json;
using Phext;

var dir = args.Length > 0 ? args[0] : Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../.."));
var vectors = Directory.GetFiles(dir, "*.phext").OrderBy(f => f, StringComparer.Ordinal).ToList();
var failures = 0;
foreach (var file in vectors)
{
    var name = Path.GetFileNameWithoutExtension(file);
    var text = Encoding.UTF8.GetString(File.ReadAllBytes(file));
    var want = JsonSerializer.Deserialize<List<Expected>>(File.ReadAllText(Path.Combine(dir, name + ".expected.json")))!;
    var got = PhextEngine.Phokenize(text).Where(p => p.Scroll.Length > 0).Select(p => new Expected(p.Coord.ToString(), p.Scroll)).ToList();
    if (!got.SequenceEqual(want))
    {
        failures++;
        var i = 0;
        while (i < got.Count && i < want.Count && got[i] == want[i]) i++;
        Console.WriteLine($"FAIL {name}: {got.Count} scrolls vs {want.Count} expected; first difference at #{i}");
        Console.WriteLine($"  got:  {(i < got.Count ? got[i] : null)}");
        Console.WriteLine($"  want: {(i < want.Count ? want[i] : null)}");
    }
}
Console.WriteLine($"csharp: {vectors.Count - failures}/{vectors.Count} vectors match");
return failures == 0 ? 0 : 1;

record Expected(string coord, string text);
