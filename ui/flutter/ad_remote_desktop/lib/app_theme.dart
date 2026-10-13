import 'package:flutter/material.dart';

ThemeData aetherDeskTheme(Brightness brightness) {
  final dark = brightness == Brightness.dark;
  const peach = Color(0xFFF7D6D0);
  const cream = Color(0xFFFCEEEB);
  const rose = Color(0xFFC7A2B3);
  const lavender = Color(0xFF7C7A9A);
  const ink = Color(0xFF2C2B3F);
  final scheme = ColorScheme.fromSeed(seedColor: rose, brightness: brightness)
      .copyWith(
        primary: dark ? rose : ink,
        onPrimary: dark ? ink : cream,
        primaryContainer: dark ? rose : peach,
        onPrimaryContainer: ink,
        secondary: dark ? peach : ink,
        onSecondary: dark ? ink : cream,
        secondaryContainer: rose,
        onSecondaryContainer: ink,
        tertiary: dark ? cream : ink,
        onTertiary: dark ? ink : cream,
        surface: dark ? ink : cream,
        onSurface: dark ? cream : ink,
        onSurfaceVariant: dark ? peach : ink,
        surfaceContainerLowest: dark ? const Color(0xFF222131) : cream,
        surfaceContainerLow: dark ? const Color(0xFF333247) : cream,
        surfaceContainer: dark ? const Color(0xFF3A394F) : peach,
        surfaceContainerHigh: dark ? const Color(0xFF434158) : peach,
        surfaceContainerHighest: dark ? const Color(0xFF4C4961) : rose,
        outline: lavender,
        outlineVariant: lavender.withValues(alpha: .45),
      );
  return ThemeData(
    brightness: brightness,
    colorScheme: scheme,
    scaffoldBackgroundColor: dark ? ink : peach,
    useMaterial3: true,
    visualDensity: VisualDensity.compact,
    inputDecorationTheme: const InputDecorationTheme(
      border: OutlineInputBorder(),
      isDense: true,
    ),
    dividerTheme: DividerThemeData(color: scheme.outlineVariant),
  );
}
