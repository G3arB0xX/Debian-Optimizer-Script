import os
import sys
import subprocess
from PIL import Image, ImageDraw, ImageFont

# Basic ANSI 16 colors map
ANSI_COLORS = {
    0: (0, 0, 0),       # Black
    1: (205, 0, 0),     # Red
    2: (0, 205, 0),     # Green
    3: (205, 205, 0),   # Yellow
    4: (0, 0, 238),     # Blue
    5: (205, 0, 205),   # Magenta
    6: (0, 205, 205),   # Cyan
    7: (229, 229, 229), # White
    90: (127, 127, 127), # Bright Black (Gray)
    91: (255, 0, 0),    # Bright Red
    92: (0, 255, 0),    # Bright Green
    93: (255, 255, 0),  # Bright Yellow
    94: (92, 92, 255),  # Bright Blue
    95: (255, 0, 255),  # Bright Magenta
    96: (0, 255, 255),  # Bright Cyan
    97: (255, 255, 255), # Bright White
}

# 256 colors map (simplified fallback)
def get_256_color(n):
    if n < 8:
        return ANSI_COLORS.get(n, (255, 255, 255))
    elif n < 16:
        return ANSI_COLORS.get(n + 82, (255, 255, 255))
    elif n < 232:
        n -= 16
        r = (n // 36) * 51
        g = ((n % 36) // 6) * 51
        b = (n % 6) * 51
        return (r, g, b)
    else:
        val = 8 + (n - 232) * 10
        return (val, val, val)

def parse_ansi_text(ansi_bytes):
    rows = []
    current_row = []
    
    fg = (229, 229, 229) # Default white/grey
    bg = (40, 40, 40)    # Default dark background
    
    i = 0
    n = len(ansi_bytes)
    while i < n:
        if ansi_bytes[i] == 27 and i + 1 < n and ansi_bytes[i+1] == ord('['):
            j = i + 2
            seq = []
            while j < n and ansi_bytes[j] != ord('m'):
                seq.append(chr(ansi_bytes[j]))
                j += 1
            if j < n and ansi_bytes[j] == ord('m'):
                seq_str = "".join(seq)
                parts = [int(p) for p in seq_str.split(';') if p.isdigit()]
                
                k = 0
                while k < len(parts):
                    p = parts[k]
                    if p == 0:
                        fg = (229, 229, 229)
                        bg = (40, 40, 40)
                    elif p == 38:
                        if k + 2 < len(parts) and parts[k+1] == 5:
                            fg = get_256_color(parts[k+2])
                            k += 2
                        elif k + 4 < len(parts) and parts[k+1] == 2:
                            fg = (parts[k+2], parts[k+3], parts[k+4])
                            k += 4
                    elif p == 48:
                        if k + 2 < len(parts) and parts[k+1] == 5:
                            bg = get_256_color(parts[k+2])
                            k += 2
                        elif k + 4 < len(parts) and parts[k+1] == 2:
                            bg = (parts[k+2], parts[k+3], parts[k+4])
                            k += 4
                    elif 30 <= p <= 37:
                        fg = ANSI_COLORS.get(p - 30, fg)
                    elif 40 <= p <= 47:
                        bg = ANSI_COLORS.get(p - 40, bg)
                    elif 90 <= p <= 97:
                        fg = ANSI_COLORS.get(p, fg)
                    elif 100 <= p <= 107:
                        bg = ANSI_COLORS.get(p - 60, bg)
                    elif p == 39:
                        fg = (229, 229, 229)
                    elif p == 49:
                        bg = (40, 40, 40)
                    k += 1
                i = j + 1
            else:
                current_row.append((chr(ansi_bytes[i]), fg, bg))
                i += 1
        elif ansi_bytes[i] == ord('\n'):
            rows.append(current_row)
            current_row = []
            i += 1
        else:
            current_row.append((chr(ansi_bytes[i]), fg, bg))
            i += 1
            
    if current_row:
        rows.append(current_row)
    return rows

def render_image(rows, font_path, output_path, char_width=9, char_height=18):
    max_cols = max(len(row) for row in rows) if rows else 0
    max_rows = len(rows)
    
    img_width = max_cols * char_width
    img_height = max_rows * char_height
    
    image = Image.new("RGB", (img_width, img_height), (40, 40, 40))
    draw = ImageDraw.Draw(image)
    
    try:
        font = ImageFont.truetype(font_path, 13)
    except Exception as e:
        print("Failed to load custom font, falling back to default:", e)
        font = ImageFont.load_default()
        
    for y, row in enumerate(rows):
        for x, cell in enumerate(row):
            char, fg_color, bg_color = cell
            draw.rectangle(
                [x * char_width, y * char_height, (x + 1) * char_width, (y + 1) * char_height],
                fill=bg_color
            )
            draw.text(
                (x * char_width, y * char_height),
                char,
                font=font,
                fill=fg_color
            )
            
    image.save(output_path)
    print(f"Screenshot successfully saved to {output_path} ({img_width}x{img_height})")

def capture_and_render(session_name, output_image_path):
    res = subprocess.run(
        ["tmux", "capture-pane", "-e", "-t", session_name, "-p"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE
    )
    if res.returncode != 0:
        print("Failed to capture tmux pane:", res.stderr.decode())
        return False
        
    ansi_data = res.stdout
    rows = parse_ansi_text(ansi_data)
    
    font_path = "/usr/share/fonts/liberation-mono-fonts/LiberationMono-Regular.ttf"
    if not os.path.exists(font_path):
        font_path = "/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Regular.ttf"
        
    render_image(rows, font_path, output_image_path, char_width=9, char_height=18)
    return True

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: python3 capture_yazi.py <tmux_session> <output_image_path>")
        sys.exit(1)
    capture_and_render(sys.argv[1], sys.argv[2])
