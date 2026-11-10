"""Regenerate synthetic reader fixtures (development only).

Dependencies: xlwt==1.3.0, openpyxl. Production uses the Rust rxls crate.
Run from the repository root: python desktop/core/tests/fixtures/generate_excel_reader.py
"""
from pathlib import Path
from io import BytesIO
import zipfile
import xml.etree.ElementTree as ET
import xlwt
from openpyxl import Workbook
from openpyxl.styles import Font

OUT = Path(__file__).resolve().parent
HEADERS = ["客户名称", "产品名称", "总箱数", "收货仓", "入库号"]
ROWS = [
    ["测试客户", "门&窗 &#65; <样品>", 2, "休斯顿", "IB00001"],
    ["测试客户", "中文货物", 3, None, None],
    ["测试客户", "黑字货物", 4, "洛杉矶", "IB00002"],
    ["测试客户", "自定义深红", 5, "洛杉矶", "IB00003"],
    ["测试客户", "珊瑚色货物", 6, "洛杉矶", "IB00004"],
]


def make_xls():
    book = xlwt.Workbook()
    book.set_colour_RGB(0x21, 192, 0, 0)
    book.set_colour_RGB(0x22, 255, 128, 128)
    old = book.add_sheet("10月")
    for c, value in enumerate(HEADERS):
        old.write(0, c, value)
    old.write(1, 1, "上月货物")
    old.write(1, 2, 9)
    sheet = book.add_sheet("11月")
    red = xlwt.easyxf("font: colour red;")
    dark_red = xlwt.easyxf("font: colour 33;")
    coral = xlwt.easyxf("font: colour 34;")
    for c, value in enumerate(HEADERS):
        sheet.write(0, c, value)
    for r, row in enumerate(ROWS, 1):
        for c, value in enumerate(row):
            if value is None or (c == 4 and r == 1):
                continue
            style = red if r == 1 or (r == 3 and c == 0) else dark_red if r == 4 else coral if r == 5 else xlwt.Style.default_style
            sheet.write(r, c, value, style)
    sheet.write_merge(1, 2, 4, 4, "IB00001", red)
    sheet.write(8, 7, True)
    sheet.write(9, 7, 123.5)
    sheet.write(10, 7, 42, xlwt.easyxf(num_format_str="00000"))
    sheet.write(11, 7, "共享字符串" * 3000)
    template = book.add_sheet("12月模板")
    for c, value in enumerate(HEADERS):
        template.write(0, c, value)
    book.add_sheet("说明").write(0, 0, "普通说明")
    book.save(str(OUT / "excel-reader.xls"))


def make_xlsx():
    book = Workbook()
    sheet = book.active
    sheet.title = "11月"
    sheet.append(HEADERS)
    for r, row in enumerate(ROWS, 2):
        sheet.append(row)
        color = "FF0000" if r == 2 else "C00000" if r == 5 else "FF8080" if r == 6 else None
        if color:
            for cell in sheet[r]:
                cell.font = Font(color=color)
    sheet["A4"].font = Font(color="FF0000")
    sheet.merge_cells("E2:E3")
    sheet["H9"] = True
    sheet["H10"] = 123.5
    sheet["H11"] = 42
    sheet["H11"].number_format = "00000"
    sheet["H12"] = "共享字符串" * 3000
    sheet["H13"] = "=SUM(C2:C6)"
    stream = BytesIO()
    book.save(stream)
    ns = {"s": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    # Test cached formulas and relationship-based sheet lookup, without relying
    # on a particular physical worksheet filename. XML handling is test-only.
    with zipfile.ZipFile(stream) as source, zipfile.ZipFile(OUT / "excel-reader.xlsx", "w", zipfile.ZIP_DEFLATED) as dest:
        for entry in source.infolist():
            data = source.read(entry.filename)
            name = entry.filename
            if name == "xl/worksheets/sheet1.xml":
                xml = ET.fromstring(data)
                xml.find(".//s:c[@r='H13']/s:v", ns).text = "20"
                data = ET.tostring(xml, encoding="utf-8")
                name = "xl/worksheets/incoming-data.xml"
            elif name == "xl/_rels/workbook.xml.rels":
                xml = ET.fromstring(data)
                for rel in xml:
                    if rel.attrib.get("Target", "").endswith("/sheet1.xml"):
                        rel.set("Target", "/xl/worksheets/incoming-data.xml")
                data = ET.tostring(xml, encoding="utf-8")
            elif name == "[Content_Types].xml":
                xml = ET.fromstring(data)
                for content in xml:
                    if content.attrib.get("PartName") == "/xl/worksheets/sheet1.xml":
                        content.set("PartName", "/xl/worksheets/incoming-data.xml")
                data = ET.tostring(xml, encoding="utf-8")
            dest.writestr(name, data)


def make_quantity_merge_fixtures():
    headers = ["客户名称", "产品名称", "总箱数", "收货仓", "入库号"]
    book = xlwt.Workbook()
    sheet = book.add_sheet("合箱样例")
    for c, value in enumerate(headers):
        sheet.write(0, c, value)
    for r, name in [(1, "户外帽子-黑色"), (2, "22钉按摩梳，200/件")]:
        sheet.write(r, 0, "测试客户")
        sheet.write(r, 1, name)
        sheet.write(r, 3, "休斯顿")
    sheet.write_merge(1, 2, 2, 2, 1)
    sheet.write_merge(1, 2, 4, 4, "IB-SHARED")
    sheet.write(3, 2, 1)
    book.save(str(OUT / "excel-quantity-merged.xls"))
    book = Workbook()
    sheet = book.active
    sheet.title = "合箱样例"
    sheet.append(headers)
    sheet.append(["测试客户", "户外帽子-黑色", 1, "休斯顿", "IB-SHARED"])
    sheet.append(["测试客户", "22钉按摩梳，200/件", None, "休斯顿", None])
    sheet.merge_cells("C2:C3")
    sheet.merge_cells("E2:E3")
    sheet.append([None, None, 1])
    book.save(OUT / "excel-quantity-merged.xlsx")


if __name__ == "__main__":
    make_xls()
    make_xlsx()
    make_quantity_merge_fixtures()
