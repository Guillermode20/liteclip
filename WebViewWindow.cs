using System;
using System.Runtime.Versioning;
using System.Windows.Forms;
using Microsoft.Web.WebView2.WinForms;

namespace smart_compressor;

[SupportedOSPlatform("windows6.1")]
public class WebViewWindow : Form
{
    private WebView2 webView;

    public WebViewWindow(string url, string title = "Smart Video Compressor")
    {
        // Configure the form
        Text = title;
        Width = 1200;
        Height = 800;
        StartPosition = FormStartPosition.CenterScreen;

        // Create and configure WebView2
        webView = new WebView2
        {
            Dock = DockStyle.Fill
        };

        Controls.Add(webView);

        // Initialize and navigate
        Load += async (sender, e) =>
        {
            try
            {
                await webView.EnsureCoreWebView2Async(null);
                
                // Enable dev tools (F12)
                webView.CoreWebView2.Settings.AreDevToolsEnabled = true;
                webView.CoreWebView2.Settings.AreDefaultContextMenusEnabled = true;
                
                // Navigate to the URL
                webView.CoreWebView2.Navigate(url);
                
                Console.WriteLine($"✓ WebView2 initialized and navigated to: {url}");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"❌ Error initializing WebView2: {ex.Message}");
                MessageBox.Show($"Error loading application:\n{ex.Message}", "Error", MessageBoxButtons.OK, MessageBoxIcon.Error);
            }
        };

        // Handle window closing
        FormClosing += (sender, e) =>
        {
            Console.WriteLine("Window closing...");
        };
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        base.OnFormClosed(e);
        webView?.Dispose();
    }
}

